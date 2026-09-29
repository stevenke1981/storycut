use std::collections::{HashMap, HashSet};

use thiserror::Error;

use crate::model::{
    AssetKind, Clip, MAX_REVISION, MAX_TICKS, Project, SCHEMA_VERSION, StreamKind, TIMEBASE,
    TrackKind, VideoAudioPolicy,
};

#[derive(Clone, Debug, PartialEq, Eq, Error)]
#[error("{message}")]
pub struct ProjectValidationError {
    pub message: String,
}

impl ProjectValidationError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoreErrorCode {
    ValidationError,
    RevisionConflict,
    IdempotencyConflict,
    UnsupportedFeature,
    LockedTrack,
    NotFound,
    Conflict,
    PathDenied,
    OverwriteDenied,
    MediaChanged,
    IoError,
    SerializationError,
}

impl CoreErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ValidationError => "INVALID_ARGUMENT",
            Self::RevisionConflict => "REVISION_CONFLICT",
            Self::IdempotencyConflict => "IDEMPOTENCY_CONFLICT",
            Self::UnsupportedFeature => "UNSUPPORTED_FEATURE",
            Self::LockedTrack => "LOCKED_TRACK",
            Self::NotFound => "NOT_FOUND",
            Self::Conflict => "DEPENDENCY_CONFLICT",
            Self::PathDenied => "PATH_DENIED",
            Self::OverwriteDenied => "OVERWRITE_DENIED",
            Self::MediaChanged => "MEDIA_CHANGED",
            Self::IoError | Self::SerializationError => "INTERNAL_ERROR",
        }
    }
}

impl std::fmt::Display for CoreErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Error)]
#[error("{code}: {message}")]
pub struct CoreError {
    pub code: CoreErrorCode,
    pub message: String,
}

impl CoreError {
    pub fn new(code: CoreErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn validation(message: impl Into<String>) -> Self {
        Self::new(CoreErrorCode::ValidationError, message)
    }

    pub fn code(&self) -> &'static str {
        self.code.as_str()
    }
}

pub fn validate_project(project: &Project) -> Result<(), ProjectValidationError> {
    if project.schema_version != SCHEMA_VERSION {
        return Err(ProjectValidationError::new("unsupported schema_version"));
    }
    validate_id("project_id", &project.project_id)?;
    if project.revision > MAX_REVISION {
        return Err(ProjectValidationError::new(
            "revision exceeds safe integer range",
        ));
    }
    if project.name.chars().count() > 65_536 {
        return Err(ProjectValidationError::new("project name is too long"));
    }
    if project.timebase != TIMEBASE {
        return Err(ProjectValidationError::new("timebase must equal 705600000"));
    }
    if !(16..=7680).contains(&project.canvas.width) || !(16..=7680).contains(&project.canvas.height)
    {
        return Err(ProjectValidationError::new(
            "canvas dimensions must be between 16 and 7680",
        ));
    }
    if !(1..=1_000_000).contains(&project.canvas.fps.num)
        || !(1..=1_000_000).contains(&project.canvas.fps.den)
        || project.frame_ticks().is_none()
    {
        return Err(ProjectValidationError::new(
            "frame rate is unsupported by the project timebase",
        ));
    }
    if !valid_color(&project.canvas.background) || project.canvas.color_mode != "sdr_bt709" {
        return Err(ProjectValidationError::new(
            "canvas color settings are invalid",
        ));
    }
    if !matches!(project.audio_sample_rate, 44_100 | 48_000 | 96_000) {
        return Err(ProjectValidationError::new(
            "audio_sample_rate must be 44100, 48000, or 96000",
        ));
    }
    if project.notes.len() > 100 || project.notes.iter().any(|n| n.chars().count() > 65_536) {
        return Err(ProjectValidationError::new("notes exceed project limits"));
    }
    if project.assets.len() > 100_000
        || project.tracks.len() > 256
        || project.clips.len() > 100_000
        || project.transitions.len() > 100_000
        || project.links.len() > 100_000
        || project.subtitles.len() > 100_000
    {
        return Err(ProjectValidationError::new(
            "project contains too many objects",
        ));
    }

    let mut ids = HashSet::new();
    let mut assets = HashMap::new();
    for asset in &project.assets {
        add_id(&mut ids, "asset", &asset.id)?;
        if asset.path.is_empty() || asset.path.chars().count() > 32_767 {
            return Err(ProjectValidationError::new(format!(
                "asset {} has an invalid path",
                asset.id
            )));
        }
        if asset
            .duration_ticks
            .is_some_and(|v| !valid_tick(v) || v == 0)
        {
            return Err(ProjectValidationError::new(format!(
                "asset {} has an invalid duration",
                asset.id
            )));
        }
        if let Some(hash) = &asset.sha256 {
            if hash.len() != 64
                || !hash
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            {
                return Err(ProjectValidationError::new(format!(
                    "asset {} has an invalid sha256",
                    asset.id
                )));
            }
        }
        if asset.streams.len() > 64 {
            return Err(ProjectValidationError::new(format!(
                "asset {} has too many streams",
                asset.id
            )));
        }
        let mut stream_ids = HashSet::new();
        for stream in &asset.streams {
            if stream.index > MAX_REVISION || !stream_ids.insert(stream.index) {
                return Err(ProjectValidationError::new(format!(
                    "asset {} has an invalid or duplicate stream index",
                    asset.id
                )));
            }
            if !(1..=1_000_000).contains(&stream.time_base.num)
                || !(1..=1_000_000).contains(&stream.time_base.den)
            {
                return Err(ProjectValidationError::new(format!(
                    "asset {} has an invalid stream time base",
                    asset.id
                )));
            }
            if stream
                .sample_rate
                .is_some_and(|rate| !(8_000..=384_000).contains(&rate))
            {
                return Err(ProjectValidationError::new(format!(
                    "asset {} has an invalid stream sample rate",
                    asset.id
                )));
            }
            if (stream.kind == StreamKind::Audio) != stream.sample_rate.is_some() {
                return Err(ProjectValidationError::new(format!(
                    "asset {} stream sample_rate does not match its kind",
                    asset.id
                )));
            }
        }
        assets.insert(asset.id.as_str(), asset);
    }

    let mut tracks = HashMap::new();
    for track in &project.tracks {
        add_id(&mut ids, "track", &track.id)?;
        if track.name.is_empty() || track.name.chars().count() > 200 {
            return Err(ProjectValidationError::new(format!(
                "track {} has an invalid name",
                track.id
            )));
        }
        if !finite_between(track.gain_db, -96.0, 24.0) {
            return Err(ProjectValidationError::new(format!(
                "track {} gain is outside -96..24 dB",
                track.id
            )));
        }
        if track.kind != TrackKind::Audio && (track.gain_db != 0.0 || track.muted || track.solo) {
            return Err(ProjectValidationError::new(format!(
                "non-audio track {} has audio-only settings",
                track.id
            )));
        }
        tracks.insert(track.id.as_str(), track);
    }

    let mut clips = HashMap::new();
    for clip in &project.clips {
        add_id(&mut ids, "clip", clip.id())?;
        let track = tracks.get(clip.track_id()).ok_or_else(|| {
            ProjectValidationError::new(format!("clip {} references missing track", clip.id()))
        })?;
        let asset = assets.get(clip.asset_id()).ok_or_else(|| {
            ProjectValidationError::new(format!("clip {} references missing asset", clip.id()))
        })?;
        let end = clip.end_tick().ok_or_else(|| {
            ProjectValidationError::new(format!("clip {} time overflows", clip.id()))
        })?;
        if clip.duration_ticks() == 0
            || clip.duration_ticks() > MAX_TICKS
            || clip.start_tick() > MAX_TICKS
            || end > MAX_TICKS
        {
            return Err(ProjectValidationError::new(format!(
                "clip {} has an invalid timeline range",
                clip.id()
            )));
        }
        if clip.source_in_tick() > MAX_TICKS {
            return Err(ProjectValidationError::new(format!(
                "clip {} source_in_tick exceeds the maximum",
                clip.id()
            )));
        }
        match clip {
            Clip::Image(image) => {
                if !matches!(track.kind, TrackKind::Video | TrackKind::Image)
                    || asset.kind != AssetKind::Image
                {
                    return Err(ProjectValidationError::new(format!(
                        "image clip {} has incompatible track or asset",
                        image.id
                    )));
                }
                if image.source_in_tick != 0 {
                    return Err(ProjectValidationError::new(format!(
                        "image clip {} source_in_tick must be zero",
                        image.id
                    )));
                }
                validate_frame_range(project, clip)?;
                validate_motion(project, &image.motion, image.duration_ticks, &image.id)?;
            }
            Clip::Video(video) => {
                if track.kind != TrackKind::Video || asset.kind != AssetKind::Video {
                    return Err(ProjectValidationError::new(format!(
                        "video clip {} has incompatible track or asset",
                        video.id
                    )));
                }
                if !asset
                    .streams
                    .iter()
                    .any(|s| s.index == video.stream_index && s.kind == StreamKind::Video)
                {
                    return Err(ProjectValidationError::new(format!(
                        "video clip {} references a missing video stream",
                        video.id
                    )));
                }
                validate_frame_range(project, clip)?;
                validate_motion(project, &video.motion, video.duration_ticks, &video.id)?;
            }
            Clip::Audio(audio) => {
                if track.kind != TrackKind::Audio || asset.kind == AssetKind::Image {
                    return Err(ProjectValidationError::new(format!(
                        "audio clip {} has incompatible track or asset",
                        audio.id
                    )));
                }
                if !asset
                    .streams
                    .iter()
                    .any(|s| s.index == audio.stream_index && s.kind == StreamKind::Audio)
                {
                    return Err(ProjectValidationError::new(format!(
                        "audio clip {} references a missing audio stream",
                        audio.id
                    )));
                }
                validate_audio(&audio.audio, audio.duration_ticks, &audio.id)?;
                let sample_ticks = TIMEBASE / u64::from(project.audio_sample_rate);
                if TIMEBASE % u64::from(project.audio_sample_rate) != 0 || sample_ticks == 0 {
                    return Err(ProjectValidationError::new(
                        "project sample rate is not exactly representable by timebase",
                    ));
                }
                if audio.start_tick % sample_ticks != 0 || audio.duration_ticks % sample_ticks != 0
                {
                    // A linked AV clip is allowed to retain exact video-frame tick edges.
                    // The link is validated below after all clips have been indexed.
                }
            }
        }
        if let Some(source_duration) = asset
            .duration_ticks
            .filter(|_| !matches!(clip, Clip::Image(_)))
        {
            let source_end = clip
                .source_in_tick()
                .checked_add(clip.duration_ticks())
                .ok_or_else(|| {
                    ProjectValidationError::new(format!(
                        "clip {} source range overflows",
                        clip.id()
                    ))
                })?;
            if source_end > source_duration {
                return Err(ProjectValidationError::new(format!(
                    "clip {} exceeds source duration",
                    clip.id()
                )));
            }
        }
        clips.insert(clip.id(), clip);
    }

    let mut linked_clips = HashSet::new();
    for link in &project.links {
        add_id(&mut ids, "link", &link.id)?;
        if link.kind != "av_sync" || link.clip_ids[0] == link.clip_ids[1] {
            return Err(ProjectValidationError::new(format!(
                "link {} has invalid kind or duplicate members",
                link.id
            )));
        }
        let a = clips.get(link.clip_ids[0].as_str()).ok_or_else(|| {
            ProjectValidationError::new(format!("link {} references missing clip", link.id))
        })?;
        let b = clips.get(link.clip_ids[1].as_str()).ok_or_else(|| {
            ProjectValidationError::new(format!("link {} references missing clip", link.id))
        })?;
        let (video, audio) = match (a, b) {
            (Clip::Video(v), Clip::Audio(a)) | (Clip::Audio(a), Clip::Video(v)) => (v, a),
            _ => {
                return Err(ProjectValidationError::new(format!(
                    "link {} must connect one video clip and one audio clip",
                    link.id
                )));
            }
        };
        if !linked_clips.insert(video.id.as_str()) || !linked_clips.insert(audio.id.as_str()) {
            return Err(ProjectValidationError::new(format!(
                "link {} reuses a linked clip",
                link.id
            )));
        }
        if video.asset_id != audio.asset_id
            || video.track_id == audio.track_id
            || video.start_tick != audio.start_tick
            || video.duration_ticks != audio.duration_ticks
            || video.source_in_tick != audio.source_in_tick
        {
            return Err(ProjectValidationError::new(format!(
                "link {} members are not synchronized",
                link.id
            )));
        }
        let asset = assets
            .get(video.asset_id.as_str())
            .expect("validated clip asset");
        if !asset
            .streams
            .iter()
            .any(|s| s.index == audio.stream_index && s.kind == StreamKind::Audio)
        {
            return Err(ProjectValidationError::new(format!(
                "link {} audio stream does not belong to the video asset",
                link.id
            )));
        }
    }
    for clip in &project.clips {
        if let Clip::Video(video) = clip {
            match (video.audio_policy, linked_clips.contains(video.id.as_str())) {
                (VideoAudioPolicy::Muted, false) | (VideoAudioPolicy::SeparateLinked, true) => {}
                _ => {
                    return Err(ProjectValidationError::new(format!(
                        "video clip {} audio_policy does not match its AV link",
                        video.id
                    )));
                }
            }
        }
        if let Clip::Audio(audio) = clip {
            let sample_ticks = TIMEBASE / u64::from(project.audio_sample_rate);
            if !linked_clips.contains(audio.id.as_str())
                && (audio.start_tick % sample_ticks != 0
                    || audio.duration_ticks % sample_ticks != 0)
            {
                return Err(ProjectValidationError::new(format!(
                    "audio clip {} must align to project samples",
                    audio.id
                )));
            }
        }
    }

    let mut transition_pairs = HashSet::new();
    for transition in &project.transitions {
        add_id(&mut ids, "transition", &transition.id)?;
        if !valid_tick(transition.start_tick)
            || transition.duration_ticks == 0
            || !valid_tick(transition.duration_ticks)
            || transition
                .start_tick
                .checked_add(transition.duration_ticks)
                .is_none_or(|end| end > MAX_TICKS)
        {
            return Err(ProjectValidationError::new(format!(
                "transition {} has an invalid range",
                transition.id
            )));
        }
        if transition.curve != "linear" || transition.audio_policy != "independent" {
            return Err(ProjectValidationError::new(format!(
                "transition {} has unsupported curve/audio policy",
                transition.id
            )));
        }
        let from = clips.get(transition.from_clip_id.as_str()).ok_or_else(|| {
            ProjectValidationError::new(format!(
                "transition {} has missing source clip",
                transition.id
            ))
        })?;
        let to = clips.get(transition.to_clip_id.as_str()).ok_or_else(|| {
            ProjectValidationError::new(format!(
                "transition {} has missing target clip",
                transition.id
            ))
        })?;
        let from_end = from.end_tick().expect("validated clip end");
        if from.track_id() != transition.track_id
            || to.track_id() != transition.track_id
            || !from.is_visual()
            || !to.is_visual()
            || from.id() == to.id()
            || transition.start_tick != to.start_tick()
            || from_end != to.start_tick().saturating_add(transition.duration_ticks)
            || !transition_pairs.insert((from.id(), to.id()))
        {
            return Err(ProjectValidationError::new(format!(
                "transition {} does not match its adjacent clip overlap",
                transition.id
            )));
        }
    }
    for (index, left) in project
        .clips
        .iter()
        .filter(|clip| clip.is_visual())
        .enumerate()
    {
        for right in project
            .clips
            .iter()
            .filter(|clip| clip.is_visual())
            .skip(index + 1)
        {
            if left.track_id() != right.track_id() {
                continue;
            }
            let (from, to) = if left.start_tick() <= right.start_tick() {
                (left, right)
            } else {
                (right, left)
            };
            if from.end_tick().expect("validated end") > to.start_tick()
                && !transition_pairs.contains(&(from.id(), to.id()))
            {
                return Err(ProjectValidationError::new(format!(
                    "clips {} and {} overlap without a transition",
                    from.id(),
                    to.id()
                )));
            }
        }
    }
    for track_id in project
        .tracks
        .iter()
        .filter(|track| matches!(track.kind, TrackKind::Video | TrackKind::Image))
        .map(|t| t.id.as_str())
    {
        let mut intervals = project
            .transitions
            .iter()
            .filter(|t| t.track_id == track_id)
            .collect::<Vec<_>>();
        intervals.sort_by_key(|t| t.start_tick);
        for pair in intervals.windows(2) {
            if pair[0].start_tick + pair[0].duration_ticks > pair[1].start_tick {
                return Err(ProjectValidationError::new(format!(
                    "track {} has overlapping transitions",
                    track_id
                )));
            }
        }
        for clip in project
            .clips
            .iter()
            .filter(|c| c.track_id() == track_id && c.is_visual())
        {
            let overlap_sum = project
                .transitions
                .iter()
                .filter(|t| {
                    t.track_id == track_id
                        && (t.from_clip_id == clip.id() || t.to_clip_id == clip.id())
                })
                .try_fold(0u64, |sum, t| sum.checked_add(t.duration_ticks))
                .ok_or_else(|| ProjectValidationError::new("transition duration overflow"))?;
            if overlap_sum > clip.duration_ticks() {
                return Err(ProjectValidationError::new(format!(
                    "clip {} has more transition overlap than its duration",
                    clip.id()
                )));
            }
        }
    }

    for subtitle in &project.subtitles {
        add_id(&mut ids, "subtitle", &subtitle.id)?;
        let track = tracks.get(subtitle.track_id.as_str()).ok_or_else(|| {
            ProjectValidationError::new(format!(
                "subtitle {} references missing track",
                subtitle.id
            ))
        })?;
        if track.kind != TrackKind::Subtitle
            || subtitle.path.is_empty()
            || subtitle.path.chars().count() > 32_767
            || subtitle.offset_tick > MAX_TICKS
            || !subtitle.preserve_unknown_syntax
        {
            return Err(ProjectValidationError::new(format!(
                "subtitle {} has invalid metadata",
                subtitle.id
            )));
        }
        if subtitle.raw_document.chars().count() > 65_536 || subtitle.cues.len() > 100_000 {
            return Err(ProjectValidationError::new(format!(
                "subtitle {} exceeds document limits",
                subtitle.id
            )));
        }
        let mut cue_ids = HashSet::new();
        for cue in &subtitle.cues {
            validate_id("cue.id", &cue.id)?;
            if !cue_ids.insert(cue.id.as_str())
                || cue.start_tick >= cue.end_tick
                || cue.end_tick > MAX_TICKS
                || cue.text.chars().count() > 65_536
                || cue.source_payload.chars().count() > 65_536
            {
                return Err(ProjectValidationError::new(format!(
                    "subtitle {} contains an invalid or duplicate cue",
                    subtitle.id
                )));
            }
            if subtitle
                .offset_tick
                .checked_add(cue.end_tick)
                .is_none_or(|end| end > MAX_TICKS)
            {
                return Err(ProjectValidationError::new(format!(
                    "subtitle {} cue time exceeds the project limit",
                    subtitle.id
                )));
            }
        }
    }

    if project.duration_ticks() > MAX_TICKS {
        return Err(ProjectValidationError::new(
            "project duration exceeds 24 hours",
        ));
    }
    Ok(())
}

fn validate_id(field: &str, value: &str) -> Result<(), ProjectValidationError> {
    let bytes = value.as_bytes();
    if bytes.is_empty()
        || bytes.len() > 80
        || !bytes[0].is_ascii_alphanumeric()
        || !bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
    {
        return Err(ProjectValidationError::new(format!(
            "{field} has an invalid identifier"
        )));
    }
    Ok(())
}

fn add_id(
    ids: &mut HashSet<String>,
    kind: &str,
    value: &str,
) -> Result<(), ProjectValidationError> {
    validate_id(kind, value)?;
    if !ids.insert(value.to_owned()) {
        return Err(ProjectValidationError::new(format!(
            "duplicate object id {value}"
        )));
    }
    Ok(())
}

fn valid_tick(value: u64) -> bool {
    value <= MAX_TICKS
}
fn valid_color(value: &str) -> bool {
    value.len() == 7 && value.starts_with('#') && value[1..].bytes().all(|b| b.is_ascii_hexdigit())
}
fn finite_between(value: f64, min: f64, max: f64) -> bool {
    value.is_finite() && value >= min && value <= max
}

fn validate_frame_range(project: &Project, clip: &Clip) -> Result<(), ProjectValidationError> {
    let frame = project.frame_ticks().expect("frame rate checked first");
    if clip.start_tick() % frame != 0
        || clip.duration_ticks() % frame != 0
        || clip.source_in_tick() % frame != 0
    {
        return Err(ProjectValidationError::new(format!(
            "clip {} must align to project frames",
            clip.id()
        )));
    }
    Ok(())
}

fn validate_motion(
    project: &Project,
    motion: &crate::model::Motion,
    clip_duration: u64,
    clip_id: &str,
) -> Result<(), ProjectValidationError> {
    let frame = project.frame_ticks().expect("frame rate checked first");
    if motion.domain_duration_ticks == 0
        || !valid_tick(motion.domain_duration_ticks)
        || motion.sample_offset_tick > MAX_TICKS
        || motion
            .sample_offset_tick
            .checked_add(clip_duration)
            .is_none_or(|end| end > motion.domain_duration_ticks)
        || motion.keyframes.is_empty()
        || motion.keyframes.len() > 2
        || !finite_between(motion.anchor.x, 0.0, 1.0)
        || !finite_between(motion.anchor.y, 0.0, 1.0)
    {
        return Err(ProjectValidationError::new(format!(
            "clip {clip_id} has invalid motion settings"
        )));
    }
    if motion.domain_duration_ticks % frame != 0 {
        return Err(ProjectValidationError::new(format!(
            "clip {clip_id} motion domain must align to project frames"
        )));
    }
    let last_tick = motion.domain_duration_ticks.checked_sub(frame).unwrap_or(0);
    let mut previous = None;
    for keyframe in &motion.keyframes {
        if !valid_tick(keyframe.tick)
            || !finite_between(keyframe.x, -4.0, 4.0)
            || !finite_between(keyframe.y, -4.0, 4.0)
            || !finite_between(keyframe.scale, f64::MIN_POSITIVE, 16.0)
            || !finite_between(keyframe.opacity, 0.0, 1.0)
            || previous.is_some_and(|p| keyframe.tick <= p)
        {
            return Err(ProjectValidationError::new(format!(
                "clip {clip_id} has invalid keyframes"
            )));
        }
        previous = Some(keyframe.tick);
    }
    if motion.keyframes[0].tick != 0
        || (motion.keyframes.len() == 1 && motion.domain_duration_ticks != frame)
        || (motion.keyframes.len() == 2 && motion.keyframes[1].tick != last_tick)
    {
        return Err(ProjectValidationError::new(format!(
            "clip {clip_id} motion keyframes must cover its frame domain"
        )));
    }
    Ok(())
}

fn validate_audio(
    audio: &crate::model::AudioSettings,
    clip_duration: u64,
    clip_id: &str,
) -> Result<(), ProjectValidationError> {
    if audio.domain_duration_ticks == 0
        || !valid_tick(audio.domain_duration_ticks)
        || audio
            .sample_offset_tick
            .checked_add(clip_duration)
            .is_none_or(|end| end > audio.domain_duration_ticks)
        || audio.sample_offset_tick > MAX_TICKS
        || !finite_between(audio.gain_db, -96.0, 24.0)
        || !finite_between(audio.pan, -1.0, 1.0)
        || audio.fade_in_ticks > audio.domain_duration_ticks
        || audio.fade_out_ticks > audio.domain_duration_ticks
        || audio.fade_in_ticks.saturating_add(audio.fade_out_ticks) > audio.domain_duration_ticks
        || audio.fade_curve != "linear_amplitude"
    {
        return Err(ProjectValidationError::new(format!(
            "audio clip {clip_id} has invalid envelope settings"
        )));
    }
    Ok(())
}

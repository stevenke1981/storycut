use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::model::{
    AudioSettings, Clip, Cue, Link, Motion, Project, Track, TrackKind, Transition, VideoAudioPolicy,
};
use crate::validation::{CoreError, CoreErrorCode, validate_project};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Transaction {
    pub project_id: String,
    pub expected_revision: u64,
    pub idempotency_key: String,
    pub dry_run: bool,
    pub operations: Vec<Operation>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApplyResult {
    pub applied: bool,
    pub dry_run: bool,
    pub base_revision: u64,
    pub revision: u64,
    pub changed_ids: Vec<String>,
    pub removed_ids: Vec<String>,
    pub duration_ticks: u64,
    /// Set when the canonical sidecar committed but the exchange JSON could not
    /// be refreshed. The sidecar remains authoritative and a later read/retry
    /// attempts to repair the projection.
    #[serde(default)]
    pub projection_warning: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, tag = "op")]
pub enum Operation {
    #[serde(rename = "track.add")]
    TrackAdd { track: Track },
    #[serde(rename = "track.update")]
    TrackUpdate {
        track_id: String,
        changes: TrackChanges,
    },
    #[serde(rename = "track.reorder")]
    TrackReorder { track_ids: Vec<String> },
    #[serde(rename = "track.remove")]
    TrackRemove {
        track_id: String,
        require_empty: bool,
    },
    #[serde(rename = "clip.add")]
    ClipAdd { clip: Clip },
    #[serde(rename = "clip.move")]
    ClipMove {
        clip_id: String,
        track_id: String,
        start_tick: u64,
        respect_links: bool,
    },
    #[serde(rename = "clip.trim")]
    ClipTrim {
        clip_id: String,
        start_tick: u64,
        source_in_tick: u64,
        duration_ticks: u64,
        keyframe_policy: KeyframePolicy,
        respect_links: bool,
    },
    #[serde(rename = "clip.split")]
    ClipSplit {
        clip_id: String,
        at_tick: u64,
        respect_links: bool,
    },
    #[serde(rename = "clip.remove")]
    ClipRemove {
        clip_id: String,
        respect_links: bool,
        mode: RemoveMode,
        ripple_track_ids: Vec<String>,
    },
    #[serde(rename = "motion.set")]
    MotionSet { clip_id: String, motion: Motion },
    #[serde(rename = "audio.set")]
    AudioSet {
        clip_id: String,
        audio: AudioSettings,
    },
    #[serde(rename = "transition.set")]
    TransitionSet { transition: Transition },
    #[serde(rename = "transition.remove")]
    TransitionRemove {
        transition_id: String,
        overlap_resolution: OverlapResolution,
        ripple_track_ids: Vec<String>,
    },
    #[serde(rename = "link.create")]
    LinkCreate { link: Link },
    #[serde(rename = "link.remove")]
    LinkRemove { link_id: String },
    #[serde(rename = "subtitle.cue.update")]
    SubtitleCueUpdate {
        document_id: String,
        cue: Cue,
        edit_mode: SubtitleEditMode,
        allow_lossy: bool,
    },
    #[serde(rename = "subtitle.cue.insert")]
    SubtitleCueInsert {
        document_id: String,
        cue: Cue,
        edit_mode: SubtitleEditMode,
        allow_lossy: bool,
    },
    #[serde(rename = "subtitle.cue.remove")]
    SubtitleCueRemove {
        document_id: String,
        cue_id: String,
        allow_lossy: bool,
    },
    #[serde(rename = "subtitle.shift")]
    SubtitleShift {
        document_id: String,
        offset_tick: u64,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrackChanges {
    #[serde(
        default,
        deserialize_with = "deserialize_present_value",
        skip_serializing_if = "Option::is_none"
    )]
    pub name: Option<String>,
    #[serde(
        default,
        deserialize_with = "deserialize_present_value",
        skip_serializing_if = "Option::is_none"
    )]
    pub locked: Option<bool>,
    #[serde(
        default,
        deserialize_with = "deserialize_present_value",
        skip_serializing_if = "Option::is_none"
    )]
    pub enabled: Option<bool>,
    #[serde(
        default,
        deserialize_with = "deserialize_present_value",
        skip_serializing_if = "Option::is_none"
    )]
    pub muted: Option<bool>,
    #[serde(
        default,
        deserialize_with = "deserialize_present_value",
        skip_serializing_if = "Option::is_none"
    )]
    pub solo: Option<bool>,
    #[serde(
        default,
        deserialize_with = "deserialize_present_value",
        skip_serializing_if = "Option::is_none"
    )]
    pub gain_db: Option<f64>,
}

fn deserialize_present_value<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyframePolicy {
    ResampleVisible,
    RetimeFull,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoveMode {
    Lift,
    Ripple,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverlapResolution {
    RejectIfOverlap,
    ButtCutRipple,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubtitleEditMode {
    TextAndTimePreserveSyntax,
    RawPayload,
}

pub(crate) fn apply_operations(
    project: &mut Project,
    operations: &[Operation],
    locked_at_start: &HashSet<String>,
) -> Result<(), CoreError> {
    if operations.is_empty() || operations.len() > 500 {
        return Err(CoreError::validation(
            "a transaction must contain 1 to 500 operations",
        ));
    }
    let link_ids = operations
        .iter()
        .filter_map(|op| match op {
            Operation::LinkCreate { link } => Some(link.id.as_str()),
            _ => None,
        })
        .collect::<HashSet<_>>();
    let transition_ids = operations
        .iter()
        .filter_map(|op| match op {
            Operation::TransitionSet { transition } => Some(transition.id.as_str()),
            _ => None,
        })
        .collect::<HashSet<_>>();
    for operation in operations {
        apply_one(project, operation, locked_at_start)?;
    }
    // Links and transitions may be added before their referenced clips within
    // the same batch. Resolve their lock effects only after every operation has
    // had a chance to create the referenced objects.
    let links = project
        .links
        .iter()
        .filter(|link| link_ids.contains(link.id.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    for link in links {
        for clip_id in &link.clip_ids {
            let clip = find_clip(project, clip_id)?.clone();
            check_track_unlocked(project, locked_at_start, clip.track_id())?;
            if let Clip::Video(video) = clip {
                if let Clip::Video(target) = find_clip_mut(project, &video.id)? {
                    target.audio_policy = VideoAudioPolicy::SeparateLinked;
                }
            }
        }
    }
    for transition in project
        .transitions
        .iter()
        .filter(|transition| transition_ids.contains(transition.id.as_str()))
    {
        check_track_unlocked(project, locked_at_start, &transition.track_id)?;
        check_track_unlocked(
            project,
            locked_at_start,
            find_clip(project, &transition.from_clip_id)?.track_id(),
        )?;
        check_track_unlocked(
            project,
            locked_at_start,
            find_clip(project, &transition.to_clip_id)?.track_id(),
        )?;
    }
    validate_project(project).map_err(|e| CoreError::validation(e.message))
}

fn apply_one(
    project: &mut Project,
    op: &Operation,
    locked: &HashSet<String>,
) -> Result<(), CoreError> {
    match op {
        Operation::TrackAdd { track } => {
            project.tracks.push(track.clone());
        }
        Operation::TrackUpdate { track_id, changes } => {
            check_track_unlocked(project, locked, track_id)?;
            let track = find_track_mut(project, track_id)?;
            if let Some(value) = &changes.name {
                track.name.clone_from(value);
            }
            if let Some(value) = changes.locked {
                track.locked = value;
            }
            if let Some(value) = changes.enabled {
                track.enabled = value;
            }
            if let Some(value) = changes.muted {
                track.muted = value;
            }
            if let Some(value) = changes.solo {
                track.solo = value;
            }
            if let Some(value) = changes.gain_db {
                track.gain_db = value;
            }
        }
        Operation::TrackReorder { track_ids } => {
            if track_ids.len() != project.tracks.len() {
                return Err(CoreError::validation(
                    "track.reorder requires every track id exactly once",
                ));
            }
            let expected = project
                .tracks
                .iter()
                .map(|t| t.id.as_str())
                .collect::<HashSet<_>>();
            let actual = track_ids.iter().map(String::as_str).collect::<HashSet<_>>();
            if expected != actual || actual.len() != track_ids.len() {
                return Err(CoreError::validation(
                    "track.reorder ids must be a complete unique permutation",
                ));
            }
            for id in track_ids {
                check_track_unlocked(project, locked, id)?;
            }
            let mut old = std::mem::take(&mut project.tracks)
                .into_iter()
                .map(|t| (t.id.clone(), t))
                .collect::<HashMap<_, _>>();
            project.tracks = track_ids
                .iter()
                .map(|id| old.remove(id).expect("permutation validated"))
                .collect();
        }
        Operation::TrackRemove {
            track_id,
            require_empty,
        } => {
            if !require_empty {
                return Err(CoreError::validation(
                    "track.remove requires require_empty=true",
                ));
            }
            check_track_unlocked(project, locked, track_id)?;
            if project.clips.iter().any(|clip| clip.track_id() == track_id)
                || project
                    .subtitles
                    .iter()
                    .any(|doc| doc.track_id == *track_id)
            {
                return Err(CoreError::new(
                    CoreErrorCode::Conflict,
                    format!("track {track_id} is not empty"),
                ));
            }
            remove_track(project, track_id)?;
        }
        Operation::ClipAdd { clip } => {
            let track_id = clip.track_id().to_owned();
            check_track_unlocked(project, locked, &track_id)?;
            if !project
                .assets
                .iter()
                .any(|asset| asset.id == clip.asset_id())
            {
                return Err(not_found("asset", clip.asset_id()));
            }
            project.clips.push(clip.clone());
        }
        Operation::ClipMove {
            clip_id,
            track_id,
            start_tick,
            respect_links,
        } => {
            if !respect_links {
                return Err(CoreError::validation(
                    "clip.move requires respect_links=true",
                ));
            }
            let index = find_clip_index(project, clip_id)?;
            let old_track = project.clips[index].track_id().to_owned();
            check_track_unlocked(project, locked, &old_track)?;
            check_track_unlocked(project, locked, track_id)?;
            validate_clip_target(&project.clips[index], find_track(project, track_id)?)?;
            let old_start = project.clips[index].start_tick();
            let delta = i128::from(*start_tick) - i128::from(old_start);
            project.clips[index].set_start_tick(*start_tick);
            set_clip_track(&mut project.clips[index], track_id);
            if let Some(linked_id) = linked_mate_id(project, clip_id) {
                let linked_index = find_clip_index(project, &linked_id)?;
                let linked_track = project.clips[linked_index].track_id().to_owned();
                check_track_unlocked(project, locked, &linked_track)?;
                let moved = shift_unsigned(project.clips[linked_index].start_tick(), delta)
                    .ok_or_else(|| CoreError::validation("linked clip move overflows"))?;
                project.clips[linked_index].set_start_tick(moved);
            }
        }
        Operation::ClipTrim {
            clip_id,
            start_tick,
            source_in_tick,
            duration_ticks,
            keyframe_policy,
            respect_links,
        } => {
            if !respect_links {
                return Err(CoreError::validation(
                    "clip.trim requires respect_links=true",
                ));
            }
            let index = find_clip_index(project, clip_id)?;
            let old = project.clips[index].clone();
            check_track_unlocked(project, locked, old.track_id())?;
            let head_delta = match &old {
                Clip::Image(_) => i128::from(*start_tick) - i128::from(old.start_tick()),
                _ => i128::from(*source_in_tick) - i128::from(old.source_in_tick()),
            };
            let frame_ticks = project.frame_ticks().unwrap_or(1);
            trim_clip(
                &mut project.clips[index],
                *start_tick,
                *source_in_tick,
                *duration_ticks,
                head_delta,
                *keyframe_policy,
                frame_ticks,
            )?;
            if let Some(linked_id) = linked_mate_id(project, clip_id) {
                let linked_index = find_clip_index(project, &linked_id)?;
                check_track_unlocked(project, locked, project.clips[linked_index].track_id())?;
                trim_clip(
                    &mut project.clips[linked_index],
                    *start_tick,
                    *source_in_tick,
                    *duration_ticks,
                    head_delta,
                    *keyframe_policy,
                    frame_ticks,
                )?;
            }
        }
        Operation::ClipSplit {
            clip_id,
            at_tick,
            respect_links,
        } => {
            if !respect_links {
                return Err(CoreError::validation(
                    "clip.split requires respect_links=true",
                ));
            }
            split_clip(project, clip_id, *at_tick, locked)?;
        }
        Operation::ClipRemove {
            clip_id,
            respect_links,
            mode,
            ripple_track_ids,
        } => {
            if !respect_links {
                return Err(CoreError::validation(
                    "clip.remove requires respect_links=true",
                ));
            }
            remove_clip(project, clip_id, *mode, ripple_track_ids, locked)?;
        }
        Operation::MotionSet { clip_id, motion } => {
            let index = find_clip_index(project, clip_id)?;
            check_track_unlocked(project, locked, project.clips[index].track_id())?;
            if let Some(target) = project.clips[index].motion_mut() {
                *target = motion.clone();
            } else {
                return Err(CoreError::validation("motion.set requires a visual clip"));
            }
        }
        Operation::AudioSet { clip_id, audio } => {
            let index = find_clip_index(project, clip_id)?;
            check_track_unlocked(project, locked, project.clips[index].track_id())?;
            if let Some(target) = project.clips[index].audio_mut() {
                *target = audio.clone();
            } else {
                return Err(CoreError::validation("audio.set requires an audio clip"));
            }
        }
        Operation::TransitionSet { transition } => {
            if project
                .tracks
                .iter()
                .any(|track| track.id == transition.track_id)
            {
                check_track_unlocked(project, locked, &transition.track_id)?;
            }
            if let Ok(from) = find_clip(project, &transition.from_clip_id) {
                check_track_unlocked(project, locked, from.track_id())?;
            }
            if let Ok(to) = find_clip(project, &transition.to_clip_id) {
                check_track_unlocked(project, locked, to.track_id())?;
            }
            if let Some(index) = project
                .transitions
                .iter()
                .position(|item| item.id == transition.id)
            {
                project.transitions[index] = transition.clone();
            } else {
                project.transitions.push(transition.clone());
            }
        }
        Operation::TransitionRemove {
            transition_id,
            overlap_resolution,
            ripple_track_ids,
        } => {
            remove_transition(
                project,
                transition_id,
                *overlap_resolution,
                ripple_track_ids,
                locked,
            )?;
        }
        Operation::LinkCreate { link } => {
            project.links.push(link.clone());
        }
        Operation::LinkRemove { link_id } => {
            let index = project
                .links
                .iter()
                .position(|link| link.id == *link_id)
                .ok_or_else(|| not_found("link", link_id))?;
            let link = project.links[index].clone();
            for id in &link.clip_ids {
                let clip = find_clip(project, id)?.clone();
                check_track_unlocked(project, locked, clip.track_id())?;
                if let Clip::Video(video) = &clip {
                    if let Clip::Video(target) = find_clip_mut(project, &video.id)? {
                        target.audio_policy = VideoAudioPolicy::Muted;
                    }
                }
                if matches!(&clip, Clip::Audio(_)) {
                    let audio_index = find_clip_index(project, id)?;
                    let sample_ticks =
                        crate::model::TIMEBASE / u64::from(project.audio_sample_rate);
                    let start_sample =
                        nearest_sample(project.clips[audio_index].start_tick(), sample_ticks)?;
                    let end_tick = project.clips[audio_index]
                        .end_tick()
                        .ok_or_else(|| CoreError::validation("linked audio end overflows"))?;
                    let end_sample = nearest_sample(end_tick, sample_ticks)?;
                    if end_sample <= start_sample {
                        return Err(CoreError::validation(
                            "unlinked audio duration is shorter than one sample",
                        ));
                    }
                    project.clips[audio_index].set_start_tick(start_sample * sample_ticks);
                    project.clips[audio_index]
                        .set_duration_ticks((end_sample - start_sample) * sample_ticks);
                }
            }
            project.links.remove(index);
        }
        Operation::SubtitleCueUpdate {
            document_id,
            cue,
            edit_mode,
            allow_lossy,
        } => {
            require_preserving_mode(*allow_lossy)?;
            let track_id = find_subtitle(project, document_id)?.track_id.clone();
            check_track_unlocked(project, locked, &track_id)?;
            let doc = find_subtitle_mut(project, document_id)?;
            let old = doc
                .cues
                .iter_mut()
                .find(|existing| existing.id == cue.id)
                .ok_or_else(|| not_found("cue", &cue.id))?;
            let mut updated = cue.clone();
            if matches!(edit_mode, SubtitleEditMode::TextAndTimePreserveSyntax) {
                updated.source_payload.clone_from(&old.source_payload);
            }
            *old = updated;
        }
        Operation::SubtitleCueInsert {
            document_id,
            cue,
            edit_mode: _,
            allow_lossy,
        } => {
            require_preserving_mode(*allow_lossy)?;
            let track_id = find_subtitle(project, document_id)?.track_id.clone();
            check_track_unlocked(project, locked, &track_id)?;
            let doc = find_subtitle_mut(project, document_id)?;
            if doc.cues.iter().any(|existing| existing.id == cue.id) {
                return Err(CoreError::new(
                    CoreErrorCode::Conflict,
                    "cue id already exists",
                ));
            }
            doc.cues.push(cue.clone());
        }
        Operation::SubtitleCueRemove {
            document_id,
            cue_id,
            allow_lossy,
        } => {
            require_preserving_mode(*allow_lossy)?;
            let track_id = find_subtitle(project, document_id)?.track_id.clone();
            check_track_unlocked(project, locked, &track_id)?;
            let doc = find_subtitle_mut(project, document_id)?;
            let index = doc
                .cues
                .iter()
                .position(|cue| cue.id == *cue_id)
                .ok_or_else(|| not_found("cue", cue_id))?;
            doc.cues.remove(index);
        }
        Operation::SubtitleShift {
            document_id,
            offset_tick,
        } => {
            let track_id = find_subtitle(project, document_id)?.track_id.clone();
            check_track_unlocked(project, locked, &track_id)?;
            let doc = find_subtitle_mut(project, document_id)?;
            doc.offset_tick = *offset_tick;
        }
    }
    Ok(())
}

fn require_preserving_mode(allow_lossy: bool) -> Result<(), CoreError> {
    if allow_lossy {
        Err(CoreError::new(
            CoreErrorCode::UnsupportedFeature,
            "lossy subtitle edits are unsupported",
        ))
    } else {
        Ok(())
    }
}

fn find_track<'a>(project: &'a Project, id: &str) -> Result<&'a Track, CoreError> {
    project
        .tracks
        .iter()
        .find(|track| track.id == id)
        .ok_or_else(|| not_found("track", id))
}
fn find_track_mut<'a>(project: &'a mut Project, id: &str) -> Result<&'a mut Track, CoreError> {
    project
        .tracks
        .iter_mut()
        .find(|track| track.id == id)
        .ok_or_else(|| not_found("track", id))
}
fn find_clip<'a>(project: &'a Project, id: &str) -> Result<&'a Clip, CoreError> {
    project
        .clips
        .iter()
        .find(|clip| clip.id() == id)
        .ok_or_else(|| not_found("clip", id))
}
fn find_clip_mut<'a>(project: &'a mut Project, id: &str) -> Result<&'a mut Clip, CoreError> {
    project
        .clips
        .iter_mut()
        .find(|clip| clip.id() == id)
        .ok_or_else(|| not_found("clip", id))
}
fn find_clip_index(project: &Project, id: &str) -> Result<usize, CoreError> {
    project
        .clips
        .iter()
        .position(|clip| clip.id() == id)
        .ok_or_else(|| not_found("clip", id))
}
fn find_subtitle_mut<'a>(
    project: &'a mut Project,
    id: &str,
) -> Result<&'a mut crate::model::Subtitle, CoreError> {
    project
        .subtitles
        .iter_mut()
        .find(|doc| doc.id == id)
        .ok_or_else(|| not_found("subtitle", id))
}
fn find_subtitle<'a>(
    project: &'a Project,
    id: &str,
) -> Result<&'a crate::model::Subtitle, CoreError> {
    project
        .subtitles
        .iter()
        .find(|doc| doc.id == id)
        .ok_or_else(|| not_found("subtitle", id))
}
fn not_found(kind: &str, id: &str) -> CoreError {
    CoreError::new(
        CoreErrorCode::NotFound,
        format!("{kind} {id} was not found"),
    )
}
fn check_track_unlocked(
    project: &Project,
    initially_locked: &HashSet<String>,
    track_id: &str,
) -> Result<(), CoreError> {
    let track = find_track(project, track_id)?;
    if initially_locked.contains(track_id) || track.locked {
        return Err(CoreError::new(
            CoreErrorCode::LockedTrack,
            format!("track {track_id} is locked"),
        ));
    }
    Ok(())
}
fn validate_clip_target(clip: &Clip, track: &Track) -> Result<(), CoreError> {
    let valid = match clip {
        Clip::Image(_) => matches!(track.kind, TrackKind::Image | TrackKind::Video),
        Clip::Video(_) => track.kind == TrackKind::Video,
        Clip::Audio(_) => track.kind == TrackKind::Audio,
    };
    if valid {
        Ok(())
    } else {
        Err(CoreError::validation(
            "clip kind cannot be moved to the target track",
        ))
    }
}
fn set_clip_track(clip: &mut Clip, id: &str) {
    match clip {
        Clip::Image(c) => c.track_id = id.to_owned(),
        Clip::Video(c) => c.track_id = id.to_owned(),
        Clip::Audio(c) => c.track_id = id.to_owned(),
    }
}
fn remove_track(project: &mut Project, id: &str) -> Result<(), CoreError> {
    let index = project
        .tracks
        .iter()
        .position(|track| track.id == id)
        .ok_or_else(|| not_found("track", id))?;
    project.tracks.remove(index);
    Ok(())
}
fn linked_mate_id(project: &Project, clip_id: &str) -> Option<String> {
    project
        .links
        .iter()
        .find(|link| link.clip_ids.iter().any(|id| id == clip_id))
        .and_then(|link| {
            link.clip_ids
                .iter()
                .find(|id| id.as_str() != clip_id)
                .cloned()
        })
}
fn shift_unsigned(value: u64, delta: i128) -> Option<u64> {
    let next = i128::from(value).checked_add(delta)?;
    u64::try_from(next).ok()
}

fn nearest_sample(tick: u64, sample_ticks: u64) -> Result<u64, CoreError> {
    let quotient = tick / sample_ticks;
    let remainder = tick % sample_ticks;
    let round_up = remainder.saturating_mul(2) > sample_ticks
        || (remainder.saturating_mul(2) == sample_ticks && quotient % 2 == 1);
    quotient
        .checked_add(u64::from(round_up))
        .ok_or_else(|| CoreError::validation("audio sample conversion overflows"))
}

fn trim_clip(
    clip: &mut Clip,
    start: u64,
    source_in: u64,
    duration: u64,
    head_delta: i128,
    policy: KeyframePolicy,
    frame_ticks: u64,
) -> Result<(), CoreError> {
    clip.set_start_tick(start);
    clip.set_source_in_tick(source_in);
    clip.set_duration_ticks(duration);
    if let Some(motion) = clip.motion_mut() {
        apply_domain_trim(motion, duration, head_delta, policy, frame_ticks)?;
    }
    if let Some(audio) = clip.audio_mut() {
        apply_audio_domain_trim(audio, duration, head_delta, policy)?;
    }
    Ok(())
}
fn apply_domain_trim(
    motion: &mut Motion,
    duration: u64,
    head_delta: i128,
    policy: KeyframePolicy,
    frame_ticks: u64,
) -> Result<(), CoreError> {
    match policy {
        KeyframePolicy::ResampleVisible => {
            motion.sample_offset_tick = shift_unsigned(motion.sample_offset_tick, head_delta)
                .filter(|value| *value <= crate::model::MAX_TICKS)
                .ok_or_else(|| {
                    CoreError::validation("motion sample offset is outside the allowed range")
                })?;
        }
        KeyframePolicy::RetimeFull => {
            let first = motion
                .keyframes
                .first()
                .cloned()
                .ok_or_else(|| CoreError::validation("motion has no keyframes"))?;
            let last = motion
                .keyframes
                .last()
                .cloned()
                .unwrap_or_else(|| first.clone());
            motion.domain_duration_ticks = duration;
            motion.sample_offset_tick = 0;
            motion.keyframes = if duration == frame_ticks {
                vec![crate::model::Keyframe { tick: 0, ..first }]
            } else {
                vec![
                    crate::model::Keyframe { tick: 0, ..first },
                    crate::model::Keyframe {
                        tick: duration - frame_ticks,
                        ..last
                    },
                ]
            };
        }
    }
    Ok(())
}
fn apply_audio_domain_trim(
    audio: &mut AudioSettings,
    duration: u64,
    head_delta: i128,
    policy: KeyframePolicy,
) -> Result<(), CoreError> {
    match policy {
        KeyframePolicy::ResampleVisible => {
            audio.sample_offset_tick = shift_unsigned(audio.sample_offset_tick, head_delta)
                .filter(|value| *value <= crate::model::MAX_TICKS)
                .ok_or_else(|| {
                    CoreError::validation("audio sample offset is outside the allowed range")
                })?;
        }
        KeyframePolicy::RetimeFull => {
            audio.domain_duration_ticks = duration;
            audio.sample_offset_tick = 0;
            if audio.fade_in_ticks > duration
                || audio.fade_out_ticks > duration
                || audio.fade_in_ticks + audio.fade_out_ticks > duration
            {
                return Err(CoreError::validation(
                    "retime_full duration is shorter than the existing audio fades",
                ));
            }
        }
    }
    Ok(())
}

fn split_clip(
    project: &mut Project,
    clip_id: &str,
    at: u64,
    locked: &HashSet<String>,
) -> Result<(), CoreError> {
    let index = find_clip_index(project, clip_id)?;
    let original = project.clips[index].clone();
    check_track_unlocked(project, locked, original.track_id())?;
    let start = original.start_tick();
    let end = original
        .end_tick()
        .ok_or_else(|| CoreError::validation("clip end overflows"))?;
    if at <= start || at >= end {
        return Err(CoreError::validation(
            "clip.split point must be strictly inside the clip",
        ));
    }
    if project.transitions.iter().any(|t| {
        (t.from_clip_id == clip_id || t.to_clip_id == clip_id)
            && at > t.start_tick
            && at < t.start_tick + t.duration_ticks
    }) {
        return Err(CoreError::new(
            CoreErrorCode::UnsupportedFeature,
            "splitting inside a transition is unsupported",
        ));
    }
    let right_id = fresh_id("clip");
    let delta = at - start;
    let mut left = original.clone();
    let mut right = original.clone();
    left.set_duration_ticks(delta);
    right.set_start_tick(at);
    right.set_duration_ticks(end - at);
    set_clip_id(&mut right, &right_id);
    match (&mut left, &mut right) {
        (Clip::Image(_), Clip::Image(b)) => {
            b.motion.sample_offset_tick = b
                .motion
                .sample_offset_tick
                .checked_add(delta)
                .ok_or_else(|| CoreError::validation("motion offset overflows"))?;
        }
        (Clip::Video(a), Clip::Video(b)) => {
            b.source_in_tick = b
                .source_in_tick
                .checked_add(delta)
                .ok_or_else(|| CoreError::validation("source offset overflows"))?;
            b.motion.sample_offset_tick = b
                .motion
                .sample_offset_tick
                .checked_add(delta)
                .ok_or_else(|| CoreError::validation("motion offset overflows"))?;
            a.duration_ticks = delta;
        }
        (Clip::Audio(a), Clip::Audio(b)) => {
            b.source_in_tick = b
                .source_in_tick
                .checked_add(delta)
                .ok_or_else(|| CoreError::validation("source offset overflows"))?;
            b.audio.sample_offset_tick = b
                .audio
                .sample_offset_tick
                .checked_add(delta)
                .ok_or_else(|| CoreError::validation("audio offset overflows"))?;
            a.duration_ticks = delta;
        }
        _ => unreachable!("split clone keeps clip kind"),
    }
    project.clips[index] = left;
    project.clips.push(right);
    if let Some(mate_id) = linked_mate_id(project, clip_id) {
        let mate_index = find_clip_index(project, &mate_id)?;
        check_track_unlocked(project, locked, project.clips[mate_index].track_id())?;
        let mate = project.clips[mate_index].clone();
        let mate_right_id = fresh_id("clip");
        let mut mate_left = mate.clone();
        let mut mate_right = mate.clone();
        mate_left.set_duration_ticks(delta);
        mate_right.set_start_tick(at);
        mate_right.set_duration_ticks(end - at);
        set_clip_id(&mut mate_right, &mate_right_id);
        if let (Clip::Audio(a), Clip::Audio(b)) = (&mut mate_left, &mut mate_right) {
            b.source_in_tick = b
                .source_in_tick
                .checked_add(delta)
                .ok_or_else(|| CoreError::validation("linked source offset overflows"))?;
            b.audio.sample_offset_tick = b
                .audio
                .sample_offset_tick
                .checked_add(delta)
                .ok_or_else(|| CoreError::validation("linked audio offset overflows"))?;
            a.duration_ticks = delta;
        } else {
            return Err(CoreError::validation("AV linked clip type is invalid"));
        }
        project.clips[mate_index] = mate_left;
        project.clips.push(mate_right);
        let link_index = project
            .links
            .iter()
            .position(|link| link.clip_ids.iter().any(|id| id == clip_id))
            .expect("mate came from link");
        let link = &mut project.links[link_index];
        for id in &mut link.clip_ids {
            if id == clip_id {
                *id = clip_id.to_owned();
            } else {
                *id = mate_id.clone();
            }
        }
        project.links.push(Link {
            id: fresh_id("link"),
            kind: "av_sync".to_owned(),
            clip_ids: [right_id, mate_right_id],
        });
    }
    Ok(())
}
fn set_clip_id(clip: &mut Clip, id: &str) {
    match clip {
        Clip::Image(c) => c.id = id.to_owned(),
        Clip::Video(c) => c.id = id.to_owned(),
        Clip::Audio(c) => c.id = id.to_owned(),
    }
}
fn fresh_id(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4().simple())
}

fn remove_clip(
    project: &mut Project,
    clip_id: &str,
    mode: RemoveMode,
    ripple_tracks: &[String],
    locked: &HashSet<String>,
) -> Result<(), CoreError> {
    let target_index = find_clip_index(project, clip_id)?;
    let target = project.clips[target_index].clone();
    check_track_unlocked(project, locked, target.track_id())?;
    let start = target.start_tick();
    let end = target
        .end_tick()
        .ok_or_else(|| CoreError::validation("clip end overflows"))?;
    let duration = end - start;
    let mut removed = HashSet::from([clip_id.to_owned()]);
    if let Some(mate) = linked_mate_id(project, clip_id) {
        check_track_unlocked(project, locked, find_clip(project, &mate)?.track_id())?;
        removed.insert(mate);
    }
    if mode == RemoveMode::Ripple {
        let requested = ripple_tracks
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        if requested.len() != ripple_tracks.len() || !requested.contains(target.track_id()) {
            return Err(CoreError::validation(
                "ripple removal must list the target track once",
            ));
        }
        for track_id in ripple_tracks {
            check_track_unlocked(project, locked, track_id)?;
        }
        for clip in &project.clips {
            if removed.contains(clip.id()) || !requested.contains(clip.track_id()) {
                continue;
            }
            let clip_end = clip
                .end_tick()
                .ok_or_else(|| CoreError::validation("clip end overflows"))?;
            if clip.start_tick() < end && clip_end > start {
                return Err(CoreError::new(
                    CoreErrorCode::Conflict,
                    format!("ripple range crosses clip {}; split it first", clip.id()),
                ));
            }
        }
        for clip in &mut project.clips {
            if !removed.contains(clip.id())
                && requested.contains(clip.track_id())
                && clip.start_tick() >= end
            {
                clip.set_start_tick(clip.start_tick() - duration);
            }
        }
        for transition in &mut project.transitions {
            if requested.contains(transition.track_id.as_str()) && transition.start_tick >= end {
                transition.start_tick -= duration;
            }
        }
    }
    project.clips.retain(|clip| !removed.contains(clip.id()));
    let removed_links = project
        .links
        .iter()
        .filter(|link| link.clip_ids.iter().any(|id| removed.contains(id)))
        .map(|link| link.id.clone())
        .collect::<HashSet<_>>();
    project
        .links
        .retain(|link| !removed_links.contains(&link.id));
    Ok(())
}

fn remove_transition(
    project: &mut Project,
    id: &str,
    resolution: OverlapResolution,
    ripple_tracks: &[String],
    locked: &HashSet<String>,
) -> Result<(), CoreError> {
    let index = project
        .transitions
        .iter()
        .position(|transition| transition.id == id)
        .ok_or_else(|| not_found("transition", id))?;
    let transition = project.transitions[index].clone();
    check_track_unlocked(project, locked, &transition.track_id)?;
    if resolution == OverlapResolution::ButtCutRipple {
        let requested = ripple_tracks
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        if requested.len() != ripple_tracks.len()
            || !requested.contains(transition.track_id.as_str())
        {
            return Err(CoreError::validation(
                "butt_cut_ripple must list the transition track once",
            ));
        }
        for track in ripple_tracks {
            check_track_unlocked(project, locked, track)?;
        }
        let incoming = find_clip(project, &transition.to_clip_id)?.clone();
        let old_start = incoming.start_tick();
        let overlap_end = old_start
            .checked_add(transition.duration_ticks)
            .ok_or_else(|| CoreError::validation("transition range overflows"))?;
        for clip in &mut project.clips {
            if requested.contains(clip.track_id())
                && clip.track_id() == transition.track_id
                && clip.start_tick() >= old_start
            {
                let new_start = clip
                    .start_tick()
                    .checked_add(transition.duration_ticks)
                    .ok_or_else(|| CoreError::validation("ripple move overflows"))?;
                clip.set_start_tick(new_start);
            }
        }
        for other in &mut project.transitions {
            if other.id != transition.id
                && other.track_id == transition.track_id
                && other.start_tick >= old_start
            {
                other.start_tick = other
                    .start_tick
                    .checked_add(transition.duration_ticks)
                    .ok_or_else(|| CoreError::validation("transition ripple overflows"))?;
            }
        }
        if let Some(mate_id) = linked_mate_id(project, &transition.to_clip_id) {
            let mate_index = find_clip_index(project, &mate_id)?;
            check_track_unlocked(project, locked, project.clips[mate_index].track_id())?;
            let new_start = project.clips[mate_index]
                .start_tick()
                .checked_add(transition.duration_ticks)
                .ok_or_else(|| CoreError::validation("linked ripple move overflows"))?;
            project.clips[mate_index].set_start_tick(new_start);
        }
        let _ = overlap_end;
    }
    project.transitions.remove(index);
    Ok(())
}

pub(crate) fn changed_and_removed_ids(
    before: &Project,
    after: &Project,
) -> Result<(Vec<String>, Vec<String>), CoreError> {
    let previous = object_map(before)?;
    let current = object_map(after)?;
    let mut changed = current
        .iter()
        .filter(|(id, value)| previous.get(*id).is_none_or(|old| old != *value))
        .map(|(id, _)| id.clone())
        .collect::<Vec<_>>();
    let mut removed = previous
        .keys()
        .filter(|id| !current.contains_key(*id))
        .cloned()
        .collect::<Vec<_>>();
    changed.sort();
    removed.sort();
    Ok((changed, removed))
}

fn object_map(project: &Project) -> Result<HashMap<String, serde_json::Value>, CoreError> {
    let mut result = HashMap::new();
    let mut add = |id: &str, value: serde_json::Value| {
        result.insert(id.to_owned(), value);
    };
    for asset in &project.assets {
        add(
            &asset.id,
            serde_json::to_value(asset)
                .map_err(|e| CoreError::new(CoreErrorCode::SerializationError, e.to_string()))?,
        );
    }
    for track in &project.tracks {
        add(
            &track.id,
            serde_json::to_value(track)
                .map_err(|e| CoreError::new(CoreErrorCode::SerializationError, e.to_string()))?,
        );
    }
    for clip in &project.clips {
        add(
            clip.id(),
            serde_json::to_value(clip)
                .map_err(|e| CoreError::new(CoreErrorCode::SerializationError, e.to_string()))?,
        );
    }
    for transition in &project.transitions {
        add(
            &transition.id,
            serde_json::to_value(transition)
                .map_err(|e| CoreError::new(CoreErrorCode::SerializationError, e.to_string()))?,
        );
    }
    for link in &project.links {
        add(
            &link.id,
            serde_json::to_value(link)
                .map_err(|e| CoreError::new(CoreErrorCode::SerializationError, e.to_string()))?,
        );
    }
    for subtitle in &project.subtitles {
        add(
            &subtitle.id,
            serde_json::to_value(subtitle)
                .map_err(|e| CoreError::new(CoreErrorCode::SerializationError, e.to_string()))?,
        );
    }
    Ok(result)
}

//! FFmpeg-backed media probing, multi-track rendering, and output verification.
//!
//! The crate consumes the serialized `storycut-core::Project` contract so the
//! renderer does not maintain a second timeline model. Source assets are only
//! opened for reading. Output is rendered to a fresh same-directory temporary
//! file, decoded and probed, then published with a no-replace hard link.

use serde::Serialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use thiserror::Error;

pub const TIMEBASE: u64 = 705_600_000;
const MAX_LAYER_PIXELS: u128 = 16_777_216;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

struct TempArtifact(PathBuf);

impl Drop for TempArtifact {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[derive(Debug, Error)]
pub enum RenderError {
    #[error("invalid project: {0}")]
    InvalidProject(String),
    #[error("unsupported render feature: {0}")]
    UnsupportedFeature(String),
    #[error("invalid render options: {0}")]
    InvalidOptions(String),
    #[error("media changed while rendering: {0}")]
    MediaChanged(PathBuf),
    #[error("output already exists: {0}")]
    OutputExists(PathBuf),
    #[error("{program} failed: {detail}")]
    ProcessFailed { program: String, detail: String },
    #[error("render output verification failed: {0}")]
    VerificationFailed(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

impl RenderError {
    /// Stable error code for the command/CLI/MCP adapters.
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidProject(_) => "INVALID_PROJECT",
            Self::UnsupportedFeature(_) => "UNSUPPORTED_FEATURE",
            Self::InvalidOptions(_) => "INVALID_OPTIONS",
            Self::MediaChanged(_) => "MEDIA_CHANGED",
            Self::OutputExists(_) => "OUTPUT_EXISTS",
            Self::ProcessFailed { .. } => "PROCESS_FAILED",
            Self::VerificationFailed(_) => "VERIFICATION_FAILED",
            Self::Io(_) | Self::Json(_) => "IO_ERROR",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetKind {
    Image,
    Video,
    Audio,
}

#[derive(Debug, Clone, Serialize)]
pub struct StreamTimeBase {
    pub num: u32,
    pub den: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProbeStream {
    pub index: u32,
    pub kind: String,
    pub time_base: StreamTimeBase,
    pub sample_rate: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProbeReport {
    pub kind: AssetKind,
    pub duration_ticks: Option<u64>,
    pub sha256: String,
    pub streams: Vec<ProbeStream>,
    #[serde(skip)]
    pub width: Option<u32>,
    #[serde(skip)]
    pub height: Option<u32>,
    #[serde(skip)]
    pub format_name: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RenderReport {
    pub output: PathBuf,
    pub output_sha256: String,
    pub project_id: String,
    pub source_revision: u64,
    pub frame_count: u64,
    pub fps_num: u32,
    pub fps_den: u32,
    pub width: u32,
    pub height: u32,
    pub audio_sample_rate: u32,
    /// Decoded sample frames, including any codec priming/padding measured from the file.
    pub audio_sample_count: u64,
    pub planned_audio_sample_count: u64,
    /// Container duration measured by ffprobe; AAC padding can make it differ slightly from the timeline duration.
    pub duration_ticks: u64,
    pub planned_duration_ticks: u64,
    pub subtitle_mode: String,
    pub verified_streams: Vec<String>,
    pub ffmpeg_version: String,
    pub ffprobe_version: String,
    pub warnings: Vec<String>,
    /// Present when `options.master_loudness` normalized the mix.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub master_loudness: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PreviewReport {
    pub output: PathBuf,
    pub output_sha256: String,
    pub project_id: String,
    pub source_revision: u64,
    pub at_tick: u64,
    pub width: u32,
    pub height: u32,
    pub include_subtitles: bool,
    pub decoded_frame_count: u64,
    pub ffmpeg_version: String,
    pub ffprobe_version: String,
}

#[derive(Debug, Clone)]
struct MediaFacts {
    report: ProbeReport,
    streams: Vec<StreamFacts>,
}

#[derive(Debug, Clone)]
struct StreamFacts {
    index: u32,
    kind: String,
    ordinal: u32,
    width: Option<u32>,
    height: Option<u32>,
    frames: Option<u64>,
}

#[derive(Debug, Clone)]
struct Asset {
    path: PathBuf,
    declared_kind: String,
}

#[derive(Debug, Clone)]
struct Track {
    id: String,
    kind: String,
    enabled: bool,
    muted: bool,
    solo: bool,
    gain_db: f64,
    order: usize,
    ducking: Option<Ducking>,
}

#[derive(Debug, Clone)]
struct Ducking {
    source_track_id: String,
    threshold: f64,
    ratio: f64,
    attack_ms: f64,
    release_ms: f64,
}

#[derive(Debug, Clone)]
struct Clip {
    id: String,
    track_id: String,
    asset_id: String,
    kind: String,
    start: u64,
    duration: u64,
    source_in: u64,
    stream_index: Option<u32>,
    motion: Option<Motion>,
    audio: Option<AudioSettings>,
    /// Frozen first-frame head / last-frame tail of a video clip.
    hold_head: u64,
    hold_tail: u64,
}

#[derive(Debug, Clone)]
struct Motion {
    fit: String,
    anchor_x: f64,
    anchor_y: f64,
    avoid_exposed_edges: bool,
    sample_offset: u64,
    interpolation: String,
    keyframes: Vec<MotionKeyframe>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct MotionKeyframe {
    tick: u64,
    x: f64,
    y: f64,
    scale: f64,
    opacity: f64,
}

impl Motion {
    fn first(&self) -> MotionKeyframe {
        self.keyframes[0]
    }

    fn is_static(&self) -> bool {
        self.keyframes
            .get(1)
            .is_none_or(|last| same_motion_values(self.first(), *last))
    }

    fn scale_is_static(&self) -> bool {
        self.keyframes
            .get(1)
            .is_none_or(|last| (self.first().scale - last.scale).abs() <= f64::EPSILON)
    }

    fn opacity_is_static(&self) -> bool {
        self.keyframes
            .get(1)
            .is_none_or(|last| (self.first().opacity - last.opacity).abs() <= f64::EPSILON)
    }

    fn all_opacity_zero(&self) -> bool {
        self.keyframes.iter().all(|key| key.opacity <= 0.0)
    }
}

fn same_motion_values(a: MotionKeyframe, b: MotionKeyframe) -> bool {
    (a.x - b.x).abs() <= f64::EPSILON
        && (a.y - b.y).abs() <= f64::EPSILON
        && (a.scale - b.scale).abs() <= f64::EPSILON
        && (a.opacity - b.opacity).abs() <= f64::EPSILON
}

fn default_motion(duration: u64, frame_ticks: u64) -> Motion {
    let first = MotionKeyframe {
        tick: 0,
        x: 0.0,
        y: 0.0,
        scale: 1.0,
        opacity: 1.0,
    };
    let keyframes = if duration <= frame_ticks {
        vec![first]
    } else {
        vec![
            first,
            MotionKeyframe {
                tick: duration - frame_ticks,
                ..first
            },
        ]
    };
    Motion {
        fit: "contain".into(),
        anchor_x: 0.5,
        anchor_y: 0.5,
        avoid_exposed_edges: false,
        sample_offset: 0,
        interpolation: "linear".into(),
        keyframes,
    }
}

#[derive(Debug, Clone)]
struct AudioSettings {
    gain_db: f64,
    pan: f64,
    muted: bool,
    domain_duration: u64,
    sample_offset: u64,
    fade_in: u64,
    fade_out: u64,
}

#[derive(Debug, Clone)]
struct Transition {
    track_id: String,
    from_clip_id: String,
    to_clip_id: String,
    start: u64,
    duration: u64,
    kind: String,
}

#[derive(Debug)]
struct ProjectSnapshot {
    project_id: String,
    revision: u64,
    width: u32,
    height: u32,
    fps_num: u32,
    fps_den: u32,
    frame_ticks: u64,
    background: String,
    audio_sample_rate: u32,
    assets: HashMap<String, Asset>,
    tracks: Vec<Track>,
    clips: Vec<Clip>,
    transitions: Vec<Transition>,
    subtitles: Vec<Value>,
}

#[derive(Debug, Clone)]
struct InputSpec {
    clip_id: String,
    path: PathBuf,
    source_in: u64,
    duration: u64,
    loop_image: bool,
    /// Present when the clip has frozen holds: how to rebuild the visible window.
    hold: Option<HoldPlan>,
}

/// Visible window of a held video clip, expressed as a source read plus
/// cloned frames: `start_pad` copies of the first read frame, then the read
/// frames, then the last read frame repeated until `window` is filled.
#[derive(Debug, Clone, Copy)]
struct HoldPlan {
    start_pad: u64,
    window: u64,
}

/// Probe a local media file with ffprobe and compute a SHA-256 fingerprint.
/// The returned `streams` objects match the project schema's stream shape.
pub fn probe_media(path: &Path) -> Result<ProbeReport, RenderError> {
    Ok(probe_facts(path)?.report)
}

/// Render one immutable serialized project snapshot to a new MP4 file.
///
/// Supported today: static and animated image/video layers, contain/cover
/// placement, alpha overlays, cut edits, declared linear cross-dissolves on a
/// single visual track, and linear mixed audio with gain/pan/fades. Motion
/// samples the original keyframe domain so split clips remain continuous. Burn
/// mode accepts basic SRT cues only. Unsupported transitions fail before
/// FFmpeg starts. Existing outputs are never replaced.
pub fn render(
    project: &Value,
    project_path: &Path,
    output: &Path,
    options: &Value,
) -> Result<RenderReport, RenderError> {
    render_impl(project, project_path, None, output, options)
}

/// Render with an explicitly authorized media root.
///
/// Relative asset paths are still resolved from the project file's parent;
/// their canonical targets must be contained by `authorized_root`. Callers
/// should pass the workspace root only after authorizing access to that folder.
pub fn render_with_root(
    project: &Value,
    project_path: &Path,
    authorized_root: &Path,
    output: &Path,
    options: &Value,
) -> Result<RenderReport, RenderError> {
    render_impl(
        project,
        project_path,
        Some(authorized_root),
        output,
        options,
    )
}

fn render_impl(
    project: &Value,
    project_path: &Path,
    authorized_root: Option<&Path>,
    output: &Path,
    options: &Value,
) -> Result<RenderReport, RenderError> {
    let snapshot = parse_project(project)?;
    let opts = parse_options(options)?;
    let (range_start, requested_end) = (opts.range_start, opts.range_end);
    let end = requested_end.unwrap_or(content_end(&snapshot)?);
    if end <= range_start {
        return Err(RenderError::InvalidOptions(
            "range_end_tick must be greater than range_start_tick".into(),
        ));
    }
    if range_start % snapshot.frame_ticks != 0 {
        return Err(RenderError::InvalidOptions(
            "range_start_tick must be aligned to the project frame grid".into(),
        ));
    }
    let span = end
        .checked_sub(range_start)
        .ok_or_else(|| RenderError::InvalidOptions("invalid render range".into()))?;
    let frame_count = div_ceil(span, snapshot.frame_ticks);
    if frame_count == 0 {
        return Err(RenderError::InvalidOptions("render range is empty".into()));
    }
    let duration_ticks = frame_count
        .checked_mul(snapshot.frame_ticks)
        .ok_or_else(|| RenderError::InvalidOptions("render duration overflow".into()))?;
    let range_start_sample = ticks_to_samples(range_start, snapshot.audio_sample_rate)?;
    let range_end_tick = range_start
        .checked_add(duration_ticks)
        .ok_or_else(|| RenderError::InvalidOptions("render sample endpoint overflow".into()))?;
    let range_end_sample = ticks_to_samples(range_end_tick, snapshot.audio_sample_rate)?;
    let sample_count = range_end_sample.saturating_sub(range_start_sample);
    const MAX_TICK: u64 = TIMEBASE * 24 * 60 * 60;
    if range_start > MAX_TICK || range_end_tick > MAX_TICK {
        return Err(RenderError::InvalidOptions(
            "render range exceeds the 24-hour project limit".into(),
        ));
    }
    validate_transition_range(&snapshot, range_start, range_end_tick)?;

    let output = absolute_path(output)?;
    if output
        .extension()
        .and_then(OsStr::to_str)
        .map(|s| s.to_ascii_lowercase())
        != Some("mp4".into())
    {
        return Err(RenderError::UnsupportedFeature(
            "this renderer currently publishes MP4 output only".into(),
        ));
    }
    let parent = output.parent().unwrap_or_else(|| Path::new("."));
    if !parent.is_dir() {
        return Err(RenderError::InvalidOptions(format!(
            "output directory does not exist: {}",
            parent.display()
        )));
    }
    if output.exists() {
        return Err(RenderError::OutputExists(output));
    }

    let required_asset_ids = render_asset_ids(&snapshot, range_start, range_end_tick)?;
    let base_dir = project_base_dir(project_path)?;
    let media_root = canonical_media_root(&base_dir, authorized_root)?;
    let mut assets = HashMap::new();
    for (id, asset) in &snapshot.assets {
        if !required_asset_ids.contains(id) {
            continue;
        }
        let resolved = resolve_source(&base_dir, &asset.path, &media_root)?;
        if !resolved.is_file() {
            return Err(RenderError::InvalidProject(format!(
                "asset {id} is not a readable file: {}",
                resolved.display()
            )));
        }
        let facts = probe_facts(&resolved)?;
        let expected_kind = match asset.declared_kind.as_str() {
            "image" => "image",
            "video" => "video",
            "audio" => "audio",
            other => {
                return Err(RenderError::InvalidProject(format!(
                    "asset {id} has unknown kind {other}"
                )));
            }
        };
        if !probe_kind_compatible(expected_kind, &facts.report.kind) {
            return Err(RenderError::InvalidProject(format!(
                "asset {id} declares kind {expected_kind}, ffprobe detected {:?}",
                facts.report.kind
            )));
        }
        assets.insert(id.clone(), (resolved, facts));
    }

    validate_transitions(&snapshot)?;
    let video_inputs = build_video_inputs(&snapshot, &assets, range_start, range_end_tick)?;
    let audio_inputs = build_audio_inputs(
        &snapshot,
        &assets,
        range_start,
        range_end_tick,
        range_start_sample,
    )?;
    let subtitles = prepare_subtitles(&snapshot, &opts.subtitle_mode, range_start, range_end_tick)?;

    let (temporary, temp_file) = create_temporary_output(parent, &output)?;
    let _temporary_guard = TempArtifact(temporary.clone());
    drop(temp_file);
    let sidecar = if let Some((contents, extension)) = subtitles {
        match write_sidecar(parent, &output, &contents, extension) {
            Ok(path) => Some(path),
            Err(error) => {
                let _ = fs::remove_file(&temporary);
                return Err(error);
            }
        }
    } else {
        None
    };

    let loudness = match opts.master_loudness {
        None => None,
        Some(target) => {
            match measure_loudness(&snapshot, &audio_inputs, sample_count, target, parent) {
                Ok(plan) => Some(plan),
                Err(error) => {
                    if let Some(path) = sidecar.as_ref() {
                        let _ = fs::remove_file(path);
                    }
                    let _ = fs::remove_file(&temporary);
                    return Err(error);
                }
            }
        }
    };
    let run_result = run_render(
        &snapshot,
        &video_inputs,
        &audio_inputs,
        sidecar.as_deref(),
        &temporary,
        range_start,
        duration_ticks,
        frame_count,
        sample_count,
        &opts.subtitle_mode,
        loudness.as_ref(),
    );
    if let Some(path) = sidecar.as_ref() {
        let _ = fs::remove_file(path);
    }
    if let Err(error) = run_result {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }

    // Check that no source was replaced while FFmpeg read it.
    for (path, original) in assets.values() {
        let after = sha256_file(path)?;
        if after != original.report.sha256 {
            let _ = fs::remove_file(&temporary);
            return Err(RenderError::MediaChanged(path.clone()));
        }
    }

    let verified = match verify_output(
        &temporary,
        &snapshot,
        frame_count,
        sample_count,
        duration_ticks,
    ) {
        Ok(verified) => verified,
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
    };
    let output_sha256 = match sha256_file(&temporary) {
        Ok(hash) => hash,
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
    };
    let ffmpeg_version = match tool_version("ffmpeg") {
        Ok(version) => version,
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
    };
    let ffprobe_version = match tool_version("ffprobe") {
        Ok(version) => version,
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
    };
    for (path, original) in assets.values() {
        if sha256_file(path)? != original.report.sha256 {
            let _ = fs::remove_file(&temporary);
            return Err(RenderError::MediaChanged(path.clone()));
        }
    }
    publish_without_overwrite(&temporary, &output)?;

    let warnings = if opts.subtitle_mode == "burn" {
        vec!["Subtitle burn: SRT renders cue timing and plain text; ASS/SSA keep their styles through libass; VTT is unsupported.".into()]
    } else {
        Vec::new()
    };
    Ok(RenderReport {
        output,
        output_sha256,
        project_id: snapshot.project_id,
        source_revision: snapshot.revision,
        frame_count,
        fps_num: snapshot.fps_num,
        fps_den: snapshot.fps_den,
        width: snapshot.width,
        height: snapshot.height,
        audio_sample_rate: snapshot.audio_sample_rate,
        audio_sample_count: verified.audio_samples,
        planned_audio_sample_count: sample_count,
        duration_ticks: verified.duration_ticks,
        planned_duration_ticks: duration_ticks,
        subtitle_mode: opts.subtitle_mode,
        verified_streams: verified.streams,
        ffmpeg_version,
        ffprobe_version,
        warnings,
        master_loudness: loudness.as_ref().map(LoudnessPlan::report),
    })
}

/// Render a single composited preview frame as PNG using the same render path.
/// `at_tick` must be aligned to the project's frame grid.
pub fn render_frame(
    project: &Value,
    project_path: &Path,
    output_png: &Path,
    at_tick: u64,
    width: u32,
    height: u32,
    include_subtitles: bool,
) -> Result<PreviewReport, RenderError> {
    render_frame_impl(
        project,
        project_path,
        None,
        output_png,
        at_tick,
        width,
        height,
        include_subtitles,
    )
}

/// Render a preview frame using an explicitly authorized media root.
/// Relative paths are interpreted from the project parent and canonicalized
/// source paths must remain under `authorized_root`.
pub fn render_frame_with_root(
    project: &Value,
    project_path: &Path,
    authorized_root: &Path,
    output_png: &Path,
    at_tick: u64,
    width: u32,
    height: u32,
    include_subtitles: bool,
) -> Result<PreviewReport, RenderError> {
    render_frame_impl(
        project,
        project_path,
        Some(authorized_root),
        output_png,
        at_tick,
        width,
        height,
        include_subtitles,
    )
}

fn render_frame_impl(
    project: &Value,
    project_path: &Path,
    authorized_root: Option<&Path>,
    output_png: &Path,
    at_tick: u64,
    width: u32,
    height: u32,
    include_subtitles: bool,
) -> Result<PreviewReport, RenderError> {
    let snapshot = parse_project(project)?;
    if width < 16 || height < 16 || width > 7680 || height > 7680 {
        return Err(RenderError::InvalidOptions(
            "preview width and height must be within 16..=7680".into(),
        ));
    }
    if width % 2 != 0 || height % 2 != 0 {
        return Err(RenderError::UnsupportedFeature(
            "h264_cpu-backed preview currently requires even output dimensions".into(),
        ));
    }
    if at_tick % snapshot.frame_ticks != 0 {
        return Err(RenderError::InvalidOptions(
            "preview at_tick must be aligned to the project frame grid".into(),
        ));
    }
    let content_end = content_end(&snapshot)?;
    if at_tick >= content_end {
        return Err(RenderError::InvalidOptions(
            "preview at_tick must fall before the project content end".into(),
        ));
    }
    let output = absolute_path(output_png)?;
    if output
        .extension()
        .and_then(OsStr::to_str)
        .map(|s| s.to_ascii_lowercase())
        != Some("png".into())
    {
        return Err(RenderError::UnsupportedFeature(
            "preview_frame output must use PNG".into(),
        ));
    }
    if output.exists() {
        return Err(RenderError::OutputExists(output));
    }
    let parent = output.parent().unwrap_or_else(|| Path::new("."));
    if !parent.is_dir() {
        return Err(RenderError::InvalidOptions(format!(
            "preview output directory does not exist: {}",
            parent.display()
        )));
    }
    let frame_end = at_tick
        .checked_add(snapshot.frame_ticks)
        .ok_or_else(|| RenderError::InvalidOptions("preview frame end overflow".into()))?;
    let mut resized_project = project.clone();
    let canvas = resized_project
        .get_mut("canvas")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| RenderError::InvalidProject("missing canvas object".into()))?;
    canvas.insert("width".into(), Value::from(width));
    canvas.insert("height".into(), Value::from(height));

    let intermediate = unique_candidate(
        parent,
        output
            .file_stem()
            .and_then(OsStr::to_str)
            .unwrap_or("preview"),
        "mp4",
    )?;
    let png_temporary = unique_candidate(
        parent,
        output
            .file_stem()
            .and_then(OsStr::to_str)
            .unwrap_or("preview"),
        "tmp.png",
    )?;
    let mut png_guard = match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&png_temporary)
    {
        Ok(file) => Some(file),
        Err(error) => return Err(RenderError::Io(error)),
    };
    drop(png_guard.take());
    let _png_guard = TempArtifact(png_temporary.clone());
    let _intermediate_guard = TempArtifact(intermediate.clone());
    let options = serde_json::json!({
        "range_start_tick":at_tick,
        "range_end_tick":frame_end,
        "subtitle_mode":if include_subtitles {"burn"} else {"none"},
        "encoder":"h264_cpu",
        "overwrite":false
    });
    let render_result = match authorized_root {
        Some(root) => render_with_root(
            &resized_project,
            project_path,
            root,
            &intermediate,
            &options,
        ),
        None => render(&resized_project, project_path, &intermediate, &options),
    };
    let render_report = match render_result {
        Ok(report) => report,
        Err(error) => {
            let _ = fs::remove_file(&png_temporary);
            return Err(error);
        }
    };
    let extraction = match Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-nostdin",
            "-v",
            "error",
            "-xerror",
            "-y",
            "-i",
        ])
        .arg(&intermediate)
        .args([
            "-map",
            "0:v:0",
            "-frames:v",
            "1",
            "-fps_mode",
            "passthrough",
            "-c:v",
            "png",
        ])
        .arg(&png_temporary)
        .output()
    {
        Ok(output) => output,
        Err(error) => return Err(RenderError::Io(error)),
    };
    let _ = fs::remove_file(&intermediate);
    if !extraction.status.success() {
        let _ = fs::remove_file(&png_temporary);
        return Err(process_error("ffmpeg preview extraction", extraction));
    }
    let probe = match probe_facts(&png_temporary) {
        Ok(probe) => probe,
        Err(error) => {
            let _ = fs::remove_file(&png_temporary);
            return Err(error);
        }
    };
    if probe.report.width != Some(width) || probe.report.height != Some(height) {
        let _ = fs::remove_file(&png_temporary);
        return Err(RenderError::VerificationFailed(format!(
            "preview image is {:?}x{:?}, expected {width}x{height}",
            probe.report.width, probe.report.height
        )));
    }
    let decoded_frames: u64 = probe
        .streams
        .iter()
        .filter(|s| s.kind == "video")
        .filter_map(|s| s.frames)
        .sum();
    if decoded_frames != 1 {
        let _ = fs::remove_file(&png_temporary);
        return Err(RenderError::VerificationFailed(format!(
            "preview contains {decoded_frames} decoded frames, expected one"
        )));
    }
    let decode_check = match Command::new("ffmpeg")
        .args(["-hide_banner", "-nostdin", "-v", "error", "-xerror", "-i"])
        .arg(&png_temporary)
        .args(["-f", "null", "-"])
        .output()
    {
        Ok(output) => output,
        Err(error) => return Err(RenderError::Io(error)),
    };
    if !decode_check.status.success() {
        let _ = fs::remove_file(&png_temporary);
        return Err(process_error(
            "ffmpeg preview decode verifier",
            decode_check,
        ));
    }
    let output_sha256 = sha256_file(&png_temporary)?;
    let ffmpeg_version = tool_version("ffmpeg")?;
    let ffprobe_version = tool_version("ffprobe")?;
    publish_without_overwrite(&png_temporary, &output)?;
    Ok(PreviewReport {
        output,
        output_sha256,
        project_id: render_report.project_id,
        source_revision: render_report.source_revision,
        at_tick,
        width,
        height,
        include_subtitles,
        decoded_frame_count: decoded_frames,
        ffmpeg_version,
        ffprobe_version,
    })
}

#[derive(Debug)]
struct RenderOptions {
    range_start: u64,
    range_end: Option<u64>,
    subtitle_mode: String,
    master_loudness: Option<LoudnessTarget>,
}

/// Explicit output option: EBU R128 normalization of the final mix.
#[derive(Debug, Clone, Copy)]
struct LoudnessTarget {
    integrated_lufs: f64,
    true_peak_db: f64,
    lra: f64,
}

/// Target plus first-pass measurements, applied with linear `loudnorm`.
#[derive(Debug, Clone)]
struct LoudnessPlan {
    target: LoudnessTarget,
    measured_i: f64,
    measured_tp: f64,
    measured_lra: f64,
    measured_thresh: f64,
    offset: f64,
}

impl LoudnessPlan {
    /// Linear gain to the target integrated loudness. When that gain would push
    /// the measured true peak over the ceiling, a transparent-as-possible
    /// limiter holds the ceiling (sample peak; noted in the report). FFmpeg's
    /// own `loudnorm linear=true` silently switches to dynamic mode in that
    /// case, which does not meet the explicit contract.
    fn gain_db(&self) -> f64 {
        self.target.integrated_lufs - self.measured_i
    }

    fn limited(&self) -> bool {
        self.measured_tp + self.gain_db() > self.target.true_peak_db
    }

    fn filter(&self) -> String {
        let mut chain = format!("volume={:.4}dB", self.gain_db());
        if self.limited() {
            chain.push_str(&format!(
                ",alimiter=limit={:.6}:attack=5:release=50:level=disabled:latency=1",
                10_f64.powf(self.target.true_peak_db / 20.0)
            ));
        }
        chain
    }

    fn report(&self) -> Value {
        serde_json::json!({
            "method": "two-pass: ebur128 measurement, then linear gain (+ sample-peak limiter when needed)",
            "applied_gain_db": self.gain_db(),
            "peak_limited": self.limited(),
            "target": {
                "integrated_lufs": self.target.integrated_lufs,
                "true_peak_db": self.target.true_peak_db,
                "lra": self.target.lra
            },
            "measured_before": {
                "integrated_lufs": self.measured_i,
                "true_peak_db": self.measured_tp,
                "lra": self.measured_lra,
                "threshold": self.measured_thresh
            },
            "target_offset_lu": self.offset
        })
    }
}

fn parse_options(value: &Value) -> Result<RenderOptions, RenderError> {
    if value.get("overwrite").and_then(Value::as_bool) == Some(true) {
        return Err(RenderError::InvalidOptions(
            "overwrite is disabled by this renderer; choose a new output path".into(),
        ));
    }
    let subtitle_mode = value
        .get("subtitle_mode")
        .and_then(Value::as_str)
        .unwrap_or("none")
        .to_owned();
    if subtitle_mode != "none" && subtitle_mode != "burn" {
        return Err(RenderError::InvalidOptions(format!(
            "unknown subtitle_mode {subtitle_mode}"
        )));
    }
    let encoder = value
        .get("encoder")
        .and_then(Value::as_str)
        .unwrap_or("h264_cpu");
    if encoder != "h264_cpu" {
        return Err(RenderError::UnsupportedFeature(format!(
            "encoder {encoder} is not enabled; h264_nvenc has not been validated in this environment"
        )));
    }
    let master_loudness = match value.get("master_loudness").filter(|v| !v.is_null()) {
        None => None,
        Some(loudness) => {
            let item = object(loudness, "master_loudness")?;
            for key in item.keys() {
                if !matches!(key.as_str(), "integrated_lufs" | "true_peak_db" | "lra") {
                    return Err(RenderError::InvalidOptions(format!(
                        "unknown master_loudness field {key}"
                    )));
                }
            }
            let integrated_lufs = number(item, "integrated_lufs")?.ok_or_else(|| {
                RenderError::InvalidOptions("master_loudness.integrated_lufs is required".into())
            })?;
            let true_peak_db = number(item, "true_peak_db")?.unwrap_or(-1.5);
            let lra = number(item, "lra")?.unwrap_or(11.0);
            validate_finite_range(
                "master_loudness.integrated_lufs",
                integrated_lufs,
                -70.0,
                -5.0,
            )?;
            validate_finite_range("master_loudness.true_peak_db", true_peak_db, -9.0, 0.0)?;
            validate_finite_range("master_loudness.lra", lra, 1.0, 50.0)?;
            Some(LoudnessTarget {
                integrated_lufs,
                true_peak_db,
                lra,
            })
        }
    };
    Ok(RenderOptions {
        range_start: optional_u64(value, "range_start_tick")?.unwrap_or(0),
        range_end: optional_u64(value, "range_end_tick")?,
        subtitle_mode,
        master_loudness,
    })
}

fn parse_project(value: &Value) -> Result<ProjectSnapshot, RenderError> {
    let obj = object(value, "project")?;
    let timebase = get_u64(obj, "timebase")?;
    if timebase != TIMEBASE {
        return Err(RenderError::InvalidProject(format!(
            "timebase must be {TIMEBASE}"
        )));
    }
    let canvas = object(
        obj.get("canvas")
            .ok_or_else(|| RenderError::InvalidProject("missing canvas".into()))?,
        "canvas",
    )?;
    let width = get_u32(canvas, "width")?;
    let height = get_u32(canvas, "height")?;
    let fps = object(
        canvas
            .get("fps")
            .ok_or_else(|| RenderError::InvalidProject("missing canvas.fps".into()))?,
        "canvas.fps",
    )?;
    let fps_num = get_u32(fps, "num")?;
    let fps_den = get_u32(fps, "den")?;
    if width < 16 || height < 16 || fps_num == 0 || fps_den == 0 {
        return Err(RenderError::InvalidProject(
            "canvas dimensions and fps must be positive and in range".into(),
        ));
    }
    if width > 7680 || height > 7680 {
        return Err(RenderError::InvalidProject(
            "canvas dimensions cannot exceed 7680 pixels".into(),
        ));
    }
    if u64::from(fps_num) > 240 * u64::from(fps_den) {
        return Err(RenderError::UnsupportedFeature(
            "frame rates above 240 fps are not supported by this renderer".into(),
        ));
    }
    if width % 2 != 0 || height % 2 != 0 {
        return Err(RenderError::UnsupportedFeature(
            "h264_cpu MP4 output requires even canvas width and height".into(),
        ));
    }
    let frame_numerator = u128::from(TIMEBASE) * u128::from(fps_den);
    if frame_numerator % u128::from(fps_num) != 0 {
        return Err(RenderError::UnsupportedFeature(
            "project frame rate does not map to an integral StoryCut tick".into(),
        ));
    }
    let frame_ticks = u64::try_from(frame_numerator / u128::from(fps_num))
        .map_err(|_| RenderError::InvalidProject("fps frame interval overflow".into()))?;
    let background = required_string(canvas, "background")?.to_owned();
    if background.len() != 7
        || !background.starts_with('#')
        || !background.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit)
    {
        return Err(RenderError::InvalidProject(
            "canvas.background must be #RRGGBB".into(),
        ));
    }
    let audio_sample_rate = get_u32(obj, "audio_sample_rate")?;
    if ![44_100, 48_000, 96_000].contains(&audio_sample_rate) {
        return Err(RenderError::InvalidProject(
            "audio_sample_rate must be 44100, 48000, or 96000".into(),
        ));
    }

    let mut assets = HashMap::new();
    for asset in array(obj, "assets")? {
        let item = object(asset, "asset")?;
        let id = required_string(item, "id")?.to_owned();
        let path = PathBuf::from(required_string(item, "path")?);
        let kind = required_string(item, "kind")?.to_owned();
        if assets
            .insert(
                id.clone(),
                Asset {
                    path,
                    declared_kind: kind,
                },
            )
            .is_some()
        {
            return Err(RenderError::InvalidProject(format!(
                "duplicate asset id {id}"
            )));
        }
    }

    let mut tracks = Vec::new();
    let mut track_ids = HashSet::new();
    for (order, track) in array(obj, "tracks")?.iter().enumerate() {
        let item = object(track, "track")?;
        let id = required_string(item, "id")?.to_owned();
        if !track_ids.insert(id.clone()) {
            return Err(RenderError::InvalidProject(format!(
                "duplicate track id {id}"
            )));
        }
        let gain = number(item, "gain_db")?.unwrap_or(0.0);
        validate_finite_range("track.gain_db", gain, -96.0, 24.0)?;
        tracks.push(Track {
            id,
            kind: required_string(item, "kind")?.to_owned(),
            enabled: item.get("enabled").and_then(Value::as_bool).unwrap_or(true),
            muted: item.get("muted").and_then(Value::as_bool).unwrap_or(false),
            solo: item.get("solo").and_then(Value::as_bool).unwrap_or(false),
            gain_db: gain,
            order,
            ducking: parse_ducking(item)?,
        });
    }

    let mut clips = Vec::new();
    let mut clip_ids = HashSet::new();
    for clip in array(obj, "clips")? {
        let item = object(clip, "clip")?;
        let id = required_string(item, "id")?.to_owned();
        if !clip_ids.insert(id.clone()) {
            return Err(RenderError::InvalidProject(format!(
                "duplicate clip id {id}"
            )));
        }
        let kind = required_string(item, "kind")?.to_owned();
        let duration = get_u64(item, "duration_ticks")?;
        let motion = item
            .get("motion")
            .map(|motion| parse_motion(motion, frame_ticks, duration))
            .transpose()?;
        let audio = item.get("audio").map(parse_audio).transpose()?;
        clips.push(Clip {
            id,
            track_id: required_string(item, "track_id")?.to_owned(),
            asset_id: required_string(item, "asset_id")?.to_owned(),
            kind,
            start: get_u64(item, "start_tick")?,
            duration,
            source_in: get_u64(item, "source_in_tick")?,
            stream_index: optional_u32(item, "stream_index")?,
            motion,
            audio,
            hold_head: optional_u64(clip, "hold_head_ticks")?.unwrap_or(0),
            hold_tail: optional_u64(clip, "hold_tail_ticks")?.unwrap_or(0),
        });
    }

    let mut transitions = Vec::new();
    for transition in array(obj, "transitions")? {
        let item = object(transition, "transition")?;
        transitions.push(Transition {
            track_id: required_string(item, "track_id")?.to_owned(),
            from_clip_id: required_string(item, "from_clip_id")?.to_owned(),
            to_clip_id: required_string(item, "to_clip_id")?.to_owned(),
            start: get_u64(item, "start_tick")?,
            duration: get_u64(item, "duration_ticks")?,
            kind: required_string(item, "kind")?.to_owned(),
        });
    }

    Ok(ProjectSnapshot {
        project_id: required_string(obj, "project_id")?.to_owned(),
        revision: get_u64(obj, "revision")?,
        width,
        height,
        fps_num,
        fps_den,
        frame_ticks,
        background,
        audio_sample_rate,
        assets,
        tracks,
        clips,
        transitions,
        subtitles: array(obj, "subtitles")?.to_vec(),
    })
}

fn parse_ducking(track: &Map<String, Value>) -> Result<Option<Ducking>, RenderError> {
    let Some(value) = track.get("ducking").filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let item = object(value, "track.ducking")?;
    let read = |name: &str, min: f64, max: f64| -> Result<f64, RenderError> {
        let value = number(item, name)?
            .ok_or_else(|| RenderError::InvalidProject(format!("ducking.{name} is required")))?;
        validate_finite_range(&format!("ducking.{name}"), value, min, max)?;
        Ok(value)
    };
    Ok(Some(Ducking {
        source_track_id: required_string(item, "source_track_id")?.to_owned(),
        threshold: read("threshold", 0.000_976_563, 1.0)?,
        ratio: read("ratio", 1.0, 20.0)?,
        attack_ms: read("attack_ms", 0.01, 2000.0)?,
        release_ms: read("release_ms", 0.01, 9000.0)?,
    }))
}

fn parse_motion(
    value: &Value,
    frame_ticks: u64,
    clip_duration: u64,
) -> Result<Motion, RenderError> {
    let motion = object(value, "motion")?;
    let domain_duration = get_u64(motion, "domain_duration_ticks")?;
    let sample_offset = get_u64(motion, "sample_offset_tick")?;
    if domain_duration == 0
        || domain_duration % frame_ticks != 0
        || sample_offset
            .checked_add(clip_duration)
            .is_none_or(|end| end > domain_duration)
    {
        return Err(RenderError::InvalidProject(
            "motion sample range must fit its frame-aligned domain".into(),
        ));
    }
    let interpolation = required_string(motion, "interpolation")?.to_owned();
    if interpolation != "linear" && interpolation != "smoothstep" {
        return Err(RenderError::InvalidProject(format!(
            "unknown motion interpolation {interpolation}"
        )));
    }
    let keyframes = motion
        .get("keyframes")
        .and_then(Value::as_array)
        .ok_or_else(|| RenderError::InvalidProject("motion.keyframes must be an array".into()))?;
    if keyframes.is_empty() || keyframes.len() > 2 {
        return Err(RenderError::InvalidProject(
            "motion requires one or two keyframes".into(),
        ));
    }
    let read_key = |key: &Value| -> Result<MotionKeyframe, RenderError> {
        let key = object(key, "keyframe")?;
        let tick = get_u64(key, "tick")?;
        let x = number(key, "x")?.unwrap_or(0.0);
        let y = number(key, "y")?.unwrap_or(0.0);
        let scale = number(key, "scale")?.unwrap_or(1.0);
        let opacity = number(key, "opacity")?.unwrap_or(1.0);
        validate_finite_range("motion.x", x, -4.0, 4.0)?;
        validate_finite_range("motion.y", y, -4.0, 4.0)?;
        validate_finite_range("motion.scale", scale, f64::MIN_POSITIVE, 16.0)?;
        validate_finite_range("motion.opacity", opacity, 0.0, 1.0)?;
        Ok(MotionKeyframe {
            tick,
            x,
            y,
            scale,
            opacity,
        })
    };
    let parsed_keyframes = keyframes
        .iter()
        .map(read_key)
        .collect::<Result<Vec<_>, _>>()?;
    let last_domain_tick = domain_duration - frame_ticks;
    if parsed_keyframes[0].tick != 0
        || (parsed_keyframes.len() == 1
            && (domain_duration != frame_ticks || clip_duration != frame_ticks))
        || (parsed_keyframes.len() == 2
            && (parsed_keyframes[1].tick != last_domain_tick
                || parsed_keyframes[1].tick <= parsed_keyframes[0].tick))
    {
        return Err(RenderError::InvalidProject(
            "motion keyframes must cover the frame-aligned domain endpoints".into(),
        ));
    }
    let anchor = object(
        motion
            .get("anchor")
            .ok_or_else(|| RenderError::InvalidProject("motion.anchor is missing".into()))?,
        "motion.anchor",
    )?;
    let anchor_x = number(anchor, "x")?.unwrap_or(0.5);
    let anchor_y = number(anchor, "y")?.unwrap_or(0.5);
    validate_finite_range("motion.anchor.x", anchor_x, 0.0, 1.0)?;
    validate_finite_range("motion.anchor.y", anchor_y, 0.0, 1.0)?;
    let fit = required_string(motion, "fit")?.to_owned();
    if fit != "contain" && fit != "cover" {
        return Err(RenderError::InvalidProject(format!(
            "unknown fit mode {fit}"
        )));
    }
    Ok(Motion {
        fit,
        anchor_x,
        anchor_y,
        avoid_exposed_edges: motion
            .get("avoid_exposed_edges")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        sample_offset,
        interpolation,
        keyframes: parsed_keyframes,
    })
}

fn parse_audio(value: &Value) -> Result<AudioSettings, RenderError> {
    let audio = object(value, "audio")?;
    let gain_db = number(audio, "gain_db")?.unwrap_or(0.0);
    let pan = number(audio, "pan")?.unwrap_or(0.0);
    validate_finite_range("audio.gain_db", gain_db, -96.0, 24.0)?;
    validate_finite_range("audio.pan", pan, -1.0, 1.0)?;
    Ok(AudioSettings {
        gain_db,
        pan,
        muted: audio.get("muted").and_then(Value::as_bool).unwrap_or(false),
        domain_duration: get_u64(audio, "domain_duration_ticks")?,
        sample_offset: get_u64(audio, "sample_offset_tick")?,
        fade_in: get_u64(audio, "fade_in_ticks")?,
        fade_out: get_u64(audio, "fade_out_ticks")?,
    })
}

fn content_end(project: &ProjectSnapshot) -> Result<u64, RenderError> {
    let mut end = 0_u64;
    for clip in &project.clips {
        end = end.max(clip.start.checked_add(clip.duration).ok_or_else(|| {
            RenderError::InvalidProject(format!("clip {} end tick overflow", clip.id))
        })?);
    }
    for subtitle in &project.subtitles {
        let sub = object(subtitle, "subtitle")?;
        let offset = get_u64(sub, "offset_tick")?;
        if let Some(cues) = sub.get("cues").and_then(Value::as_array) {
            for cue in cues {
                let cue = object(cue, "cue")?;
                let cue_end = get_u64(cue, "end_tick")?;
                end = end.max(offset.checked_add(cue_end).ok_or_else(|| {
                    RenderError::InvalidProject("subtitle end tick overflow".into())
                })?);
            }
        }
    }
    if end == 0 {
        return Err(RenderError::InvalidProject(
            "project has no content in the requested range".into(),
        ));
    }
    Ok(end)
}

fn validate_transitions(project: &ProjectSnapshot) -> Result<(), RenderError> {
    let track_by_id: HashMap<_, _> = project.tracks.iter().map(|t| (t.id.as_str(), t)).collect();
    let clip_by_id: HashMap<_, _> = project.clips.iter().map(|c| (c.id.as_str(), c)).collect();
    for transition in &project.transitions {
        if transition.kind != "cross_dissolve" {
            return Err(RenderError::UnsupportedFeature(format!(
                "transition {} ({}) is not implemented; only cross_dissolve is supported",
                transition.from_clip_id, transition.kind
            )));
        }
        let from = clip_by_id
            .get(transition.from_clip_id.as_str())
            .ok_or_else(|| {
                RenderError::InvalidProject("transition source clip is missing".into())
            })?;
        let to = clip_by_id
            .get(transition.to_clip_id.as_str())
            .ok_or_else(|| {
                RenderError::InvalidProject("transition target clip is missing".into())
            })?;
        if from.track_id != transition.track_id || to.track_id != transition.track_id {
            return Err(RenderError::InvalidProject(
                "transition clips and transition must belong to one track".into(),
            ));
        }
        let from_end = from.start.checked_add(from.duration).ok_or_else(|| {
            RenderError::InvalidProject("transition source end tick overflow".into())
        })?;
        let overlap = from_end.saturating_sub(to.start);
        if overlap != transition.duration || transition.start != to.start || overlap == 0 {
            return Err(RenderError::InvalidProject(
                "cross_dissolve must exactly match the declared clip overlap".into(),
            ));
        }
    }
    for track in project
        .tracks
        .iter()
        .filter(|t| t.kind == "video" || t.kind == "image")
    {
        let mut clips: Vec<_> = project
            .clips
            .iter()
            .filter(|c| c.track_id == track.id && (c.kind == "image" || c.kind == "video"))
            .collect();
        clips.sort_by_key(|clip| clip.start);
        for pair in clips.windows(2) {
            let left = pair[0];
            let right = pair[1];
            let left_end = left.start.checked_add(left.duration).ok_or_else(|| {
                RenderError::InvalidProject("visual clip end tick overflow".into())
            })?;
            if right.start < left_end {
                let valid = project.transitions.iter().any(|t| {
                    t.track_id == track.id
                        && t.from_clip_id == left.id
                        && t.to_clip_id == right.id
                        && t.start == right.start
                        && t.duration == left_end - right.start
                        && t.kind == "cross_dissolve"
                });
                if !valid {
                    return Err(RenderError::InvalidProject(format!(
                        "overlap between clips {} and {} requires an exact cross_dissolve",
                        left.id, right.id
                    )));
                }
            }
        }
    }
    let _ = track_by_id;
    Ok(())
}

fn validate_transition_range(
    project: &ProjectSnapshot,
    range_start: u64,
    range_end: u64,
) -> Result<(), RenderError> {
    for transition in &project.transitions {
        let transition_end = transition
            .start
            .checked_add(transition.duration)
            .ok_or_else(|| RenderError::InvalidProject("transition end tick overflow".into()))?;
        let cuts_transition = (range_start > transition.start && range_start < transition_end)
            || (range_end > transition.start && range_end < transition_end);
        if cuts_transition {
            return Err(RenderError::UnsupportedFeature(format!(
                "render range cuts through cross_dissolve {}..{}; render the full transition or choose a range outside it",
                transition.start, transition_end
            )));
        }
    }
    Ok(())
}

/// Collects only source assets that can contribute to the requested render
/// range. Keep these predicates aligned with `build_video_inputs` and
/// `build_audio_inputs`: unused and disabled media must not be opened merely
/// because it is listed in the project.
fn render_asset_ids(
    project: &ProjectSnapshot,
    range_start: u64,
    range_end: u64,
) -> Result<HashSet<String>, RenderError> {
    let tracks: HashMap<_, _> = project
        .tracks
        .iter()
        .map(|track| (track.id.as_str(), track))
        .collect();
    let mut asset_ids = HashSet::new();

    for clip in project
        .clips
        .iter()
        .filter(|clip| clip.kind == "image" || clip.kind == "video")
    {
        let track = tracks.get(clip.track_id.as_str()).ok_or_else(|| {
            RenderError::InvalidProject(format!(
                "clip {} refers to missing track {}",
                clip.id, clip.track_id
            ))
        })?;
        if !track.enabled || (track.kind != "video" && track.kind != "image") {
            continue;
        }
        let clip_end = clip.start.checked_add(clip.duration).ok_or_else(|| {
            RenderError::InvalidProject(format!("clip {} end tick overflow", clip.id))
        })?;
        if clip.start.max(range_start) >= clip_end.min(range_end) {
            continue;
        }
        let motion = clip
            .motion
            .clone()
            .unwrap_or_else(|| default_motion(clip.duration, project.frame_ticks));
        if !motion.all_opacity_zero() {
            asset_ids.insert(clip.asset_id.clone());
        }
    }

    let solo = project
        .tracks
        .iter()
        .any(|track| track.kind == "audio" && track.enabled && track.solo);
    for clip in project.clips.iter().filter(|clip| clip.kind == "audio") {
        let track = tracks.get(clip.track_id.as_str()).ok_or_else(|| {
            RenderError::InvalidProject(format!(
                "clip {} refers to missing track {}",
                clip.id, clip.track_id
            ))
        })?;
        if track.kind != "audio" || !track.enabled || track.muted || (solo && !track.solo) {
            continue;
        }
        let settings = clip.audio.as_ref().ok_or_else(|| {
            RenderError::InvalidProject(format!("audio clip {} has no audio settings", clip.id))
        })?;
        if settings.muted {
            continue;
        }
        if settings.sample_offset > settings.domain_duration
            || settings.fade_in > settings.domain_duration
            || settings.fade_out > settings.domain_duration
        {
            return Err(RenderError::InvalidProject(format!(
                "audio clip {} has invalid envelope domain",
                clip.id
            )));
        }
        let clip_end = clip.start.checked_add(clip.duration).ok_or_else(|| {
            RenderError::InvalidProject(format!("audio clip {} end tick overflow", clip.id))
        })?;
        let visible_start = clip.start.max(range_start);
        let visible_end = clip_end.min(range_end);
        if visible_start >= visible_end {
            continue;
        }
        let sample_offset = settings
            .sample_offset
            .checked_add(visible_start - clip.start)
            .ok_or_else(|| {
                RenderError::InvalidProject(format!(
                    "audio clip {} envelope sample offset overflow",
                    clip.id
                ))
            })?;
        if sample_offset
            .checked_add(visible_end - visible_start)
            .is_none_or(|end| end > settings.domain_duration)
        {
            return Err(RenderError::InvalidProject(format!(
                "audio clip {} visible range exceeds its envelope domain",
                clip.id
            )));
        }
        asset_ids.insert(clip.asset_id.clone());
    }

    Ok(asset_ids)
}

fn build_video_inputs(
    project: &ProjectSnapshot,
    assets: &HashMap<String, (PathBuf, MediaFacts)>,
    range_start: u64,
    range_end: u64,
) -> Result<Vec<(InputSpec, usize, usize, usize, Motion, Option<u64>)>, RenderError> {
    let mut result = Vec::new();
    let track_order: HashMap<_, _> = project
        .tracks
        .iter()
        .map(|t| (t.id.as_str(), t.order))
        .collect();
    let tracks: HashMap<_, _> = project.tracks.iter().map(|t| (t.id.as_str(), t)).collect();
    let mut clips: Vec<_> = project
        .clips
        .iter()
        .filter(|c| c.kind == "image" || c.kind == "video")
        .collect();
    clips.sort_by_key(|c| {
        (
            track_order
                .get(c.track_id.as_str())
                .copied()
                .unwrap_or(usize::MAX),
            c.start,
        )
    });
    for clip in clips {
        let track = tracks.get(clip.track_id.as_str()).ok_or_else(|| {
            RenderError::InvalidProject(format!(
                "clip {} refers to missing track {}",
                clip.id, clip.track_id
            ))
        })?;
        if !track.enabled || (track.kind != "video" && track.kind != "image") {
            continue;
        }
        let clip_end = clip.start.checked_add(clip.duration).ok_or_else(|| {
            RenderError::InvalidProject(format!("clip {} end tick overflow", clip.id))
        })?;
        let visible_start = clip.start.max(range_start);
        let visible_end = clip_end.min(range_end);
        if visible_start >= visible_end {
            continue;
        }
        let motion = clip
            .motion
            .clone()
            .unwrap_or_else(|| default_motion(clip.duration, project.frame_ticks));
        if motion.all_opacity_zero() {
            continue;
        }
        let (path, facts) = assets.get(&clip.asset_id).ok_or_else(|| {
            RenderError::InvalidProject(format!(
                "clip {} refers to missing asset {}",
                clip.id, clip.asset_id
            ))
        })?;
        let expected_stream_kind = if clip.kind == "image" {
            "video"
        } else {
            "video"
        };
        let requested_stream = clip.stream_index.unwrap_or_else(|| {
            facts
                .streams
                .iter()
                .find(|s| s.kind == expected_stream_kind)
                .map_or(0, |s| s.index)
        });
        let (stream_ordinal, source_width, source_height) =
            find_stream(facts, requested_stream, expected_stream_kind)?;
        if source_width == 0 || source_height == 0 {
            return Err(RenderError::InvalidProject(format!(
                "clip {} video dimensions are missing",
                clip.id
            )));
        }
        validate_motion_edges(
            &motion,
            source_width as f64,
            source_height as f64,
            project.width,
            project.height,
        )?;
        validate_layer_pixel_budget(
            &motion,
            source_width,
            source_height,
            project.width,
            project.height,
            &clip.id,
        )?;
        let visible_duration = visible_end - visible_start;
        let (source_offset, read_duration, hold) = if clip.kind == "video"
            && (clip.hold_head != 0 || clip.hold_tail != 0)
        {
            hold_read_window(clip, visible_start, visible_end, project.frame_ticks)?
        } else {
            let offset = clip
                .source_in
                .checked_add(visible_start - clip.start)
                .ok_or_else(|| {
                    RenderError::InvalidProject(format!("clip {} source offset overflow", clip.id))
                })?;
            (offset, visible_duration, None)
        };
        validate_source_range(clip, facts, source_offset, read_duration)?;
        let transition_fade = project
            .transitions
            .iter()
            .find(|t| {
                t.to_clip_id == clip.id && t.track_id == clip.track_id && t.kind == "cross_dissolve"
            })
            .map(|t| t.duration);
        let input = InputSpec {
            clip_id: clip.id.clone(),
            path: path.clone(),
            source_in: source_offset,
            duration: read_duration,
            loop_image: clip.kind == "image",
            hold,
        };
        result.push((
            input,
            stream_ordinal,
            source_width as usize,
            source_height as usize,
            motion,
            transition_fade,
        ));
    }
    Ok(result)
}

fn build_audio_inputs(
    project: &ProjectSnapshot,
    assets: &HashMap<String, (PathBuf, MediaFacts)>,
    range_start: u64,
    range_end: u64,
    range_start_sample: u64,
) -> Result<Vec<(InputSpec, usize, Track, AudioSettings, u64, u64)>, RenderError> {
    let tracks: HashMap<_, _> = project.tracks.iter().map(|t| (t.id.as_str(), t)).collect();
    let solo = project
        .tracks
        .iter()
        .any(|t| t.kind == "audio" && t.enabled && t.solo);
    let mut result = Vec::new();
    let mut clips: Vec<_> = project.clips.iter().filter(|c| c.kind == "audio").collect();
    clips.sort_by_key(|c| c.start);
    for clip in clips {
        let track = tracks.get(clip.track_id.as_str()).ok_or_else(|| {
            RenderError::InvalidProject(format!(
                "clip {} refers to missing track {}",
                clip.id, clip.track_id
            ))
        })?;
        if track.kind != "audio" || !track.enabled || track.muted || (solo && !track.solo) {
            continue;
        }
        let mut settings = clip.audio.clone().ok_or_else(|| {
            RenderError::InvalidProject(format!("audio clip {} has no audio settings", clip.id))
        })?;
        if settings.muted {
            continue;
        }
        if settings.sample_offset > settings.domain_duration
            || settings.fade_in > settings.domain_duration
            || settings.fade_out > settings.domain_duration
        {
            return Err(RenderError::InvalidProject(format!(
                "audio clip {} has invalid envelope domain",
                clip.id
            )));
        }
        let clip_end = clip.start.checked_add(clip.duration).ok_or_else(|| {
            RenderError::InvalidProject(format!("audio clip {} end tick overflow", clip.id))
        })?;
        let visible_start = clip.start.max(range_start);
        let visible_end = clip_end.min(range_end);
        if visible_start >= visible_end {
            continue;
        }
        let (path, facts) = assets.get(&clip.asset_id).ok_or_else(|| {
            RenderError::InvalidProject(format!(
                "clip {} refers to missing asset {}",
                clip.id, clip.asset_id
            ))
        })?;
        let requested_stream = clip.stream_index.ok_or_else(|| {
            RenderError::InvalidProject(format!("audio clip {} has no stream_index", clip.id))
        })?;
        let (stream_ordinal, _, _) = find_stream(facts, requested_stream, "audio")?;
        let source_offset = clip
            .source_in
            .checked_add(visible_start - clip.start)
            .ok_or_else(|| {
                RenderError::InvalidProject(format!("clip {} source offset overflow", clip.id))
            })?;
        let visible_duration = visible_end - visible_start;
        settings.sample_offset = settings
            .sample_offset
            .checked_add(visible_start - clip.start)
            .ok_or_else(|| {
                RenderError::InvalidProject(format!(
                    "audio clip {} envelope sample offset overflow",
                    clip.id
                ))
            })?;
        if settings
            .sample_offset
            .checked_add(visible_duration)
            .is_none_or(|end| end > settings.domain_duration)
        {
            return Err(RenderError::InvalidProject(format!(
                "audio clip {} visible range exceeds its envelope domain",
                clip.id
            )));
        }
        validate_source_range(clip, facts, source_offset, visible_duration)?;
        let absolute_start_sample = ticks_to_samples(visible_start, project.audio_sample_rate)?;
        let absolute_end_sample = ticks_to_samples(visible_end, project.audio_sample_rate)?;
        let start_samples = absolute_start_sample.saturating_sub(range_start_sample);
        let end_samples = absolute_end_sample.saturating_sub(range_start_sample);
        let input = InputSpec {
            clip_id: clip.id.clone(),
            path: path.clone(),
            source_in: source_offset,
            duration: visible_duration,
            loop_image: false,
            hold: None,
        };
        result.push((
            input,
            stream_ordinal,
            (**track).clone(),
            settings,
            start_samples,
            end_samples,
        ));
    }
    Ok(result)
}

/// Maps the visible timeline window of a held video clip to a source read.
/// Returns `(source_in, read_duration, plan)`. When the window lies entirely
/// inside a hold, a single boundary frame is read and cloned.
fn hold_read_window(
    clip: &Clip,
    visible_start: u64,
    visible_end: u64,
    frame_ticks: u64,
) -> Result<(u64, u64, Option<HoldPlan>), RenderError> {
    let span = clip
        .duration
        .checked_sub(clip.hold_head)
        .and_then(|rest| rest.checked_sub(clip.hold_tail))
        .filter(|span| *span >= frame_ticks)
        .ok_or_else(|| {
            RenderError::InvalidProject(format!(
                "video clip {} holds leave no source frames",
                clip.id
            ))
        })?;
    let a = visible_start - clip.start;
    let b = visible_end - clip.start;
    let head = clip.hold_head;
    let src_a = a.saturating_sub(head).min(span);
    let src_b = b.saturating_sub(head).min(span);
    let window = b - a;
    let (read_in, read_len, start_pad) = if src_b > src_a {
        (src_a, src_b - src_a, head.saturating_sub(a).min(window))
    } else if a < head {
        (0, frame_ticks, 0)
    } else {
        (span - frame_ticks, frame_ticks, 0)
    };
    let source_in = clip.source_in.checked_add(read_in).ok_or_else(|| {
        RenderError::InvalidProject(format!("clip {} source offset overflow", clip.id))
    })?;
    Ok((source_in, read_len, Some(HoldPlan { start_pad, window })))
}

fn validate_source_range(
    clip: &Clip,
    facts: &MediaFacts,
    source_in: u64,
    duration: u64,
) -> Result<(), RenderError> {
    if let Some(asset_duration) = facts.report.duration_ticks {
        let end = source_in
            .checked_add(duration)
            .ok_or_else(|| RenderError::InvalidProject("source range overflow".into()))?;
        // Stream durations are often rounded to milliseconds. Allow one source frame/sample of probe rounding.
        let tolerance = if clip.kind == "audio" {
            TIMEBASE / 8_000
        } else {
            TIMEBASE / 10
        };
        if end > asset_duration.saturating_add(tolerance) {
            return Err(RenderError::InvalidProject(format!(
                "clip {} requests source ticks {}..{} beyond probed duration {}",
                clip.id, source_in, end, asset_duration
            )));
        }
    }
    Ok(())
}

fn find_stream(
    facts: &MediaFacts,
    stream_index: u32,
    expected: &str,
) -> Result<(usize, u32, u32), RenderError> {
    let stream = facts
        .streams
        .iter()
        .find(|s| s.index == stream_index)
        .ok_or_else(|| {
            RenderError::InvalidProject(format!(
                "stream index {stream_index} was not found by ffprobe"
            ))
        })?;
    if stream.kind != expected {
        return Err(RenderError::InvalidProject(format!(
            "stream index {stream_index} is {}, expected {expected}",
            stream.kind
        )));
    }
    Ok((
        stream.ordinal as usize,
        stream.width.unwrap_or(0),
        stream.height.unwrap_or(0),
    ))
}

fn prepare_subtitles(
    project: &ProjectSnapshot,
    mode: &str,
    range_start: u64,
    range_end: u64,
) -> Result<Option<(String, &'static str)>, RenderError> {
    if mode == "none" {
        return Ok(None);
    }
    if let Some(document) = prepare_styled_subtitles(project)? {
        return Ok(Some((document, "ass")));
    }
    let tracks: HashMap<_, _> = project.tracks.iter().map(|t| (t.id.as_str(), t)).collect();
    let active_tracks: HashSet<_> = project
        .tracks
        .iter()
        .filter(|t| t.kind == "subtitle" && t.enabled)
        .map(|t| t.id.as_str())
        .collect();
    let mut cues: Vec<(u64, u64, String)> = Vec::new();
    for subtitle in &project.subtitles {
        let sub = object(subtitle, "subtitle")?;
        let track_id = required_string(sub, "track_id")?;
        if !active_tracks.contains(track_id) {
            continue;
        }
        let track = tracks.get(track_id).ok_or_else(|| {
            RenderError::InvalidProject(format!("subtitle track {track_id} is missing"))
        })?;
        if track.kind != "subtitle" {
            return Err(RenderError::InvalidProject(format!(
                "track {track_id} is not a subtitle track"
            )));
        }
        let format = required_string(sub, "format")?;
        if format != "srt" {
            return Err(RenderError::UnsupportedFeature(format!(
                "burn mode currently supports basic SRT only; {format} positioning/effects are not silently flattened"
            )));
        }
        if sub
            .get("cues")
            .and_then(Value::as_array)
            .is_some_and(|cues| {
                cues.iter().any(|cue| {
                    cue.get("text").and_then(Value::as_str).is_some_and(|text| {
                        text.contains('<') || text.contains('{') || text.contains('}')
                    })
                })
            })
        {
            return Err(RenderError::UnsupportedFeature(
                "SRT cue markup or positioning is not supported by burn mode".into(),
            ));
        }
        let offset = get_u64(sub, "offset_tick")?;
        if let Some(items) = sub.get("cues").and_then(Value::as_array) {
            for item in items {
                let cue = object(item, "cue")?;
                let start = offset
                    .checked_add(get_u64(cue, "start_tick")?)
                    .ok_or_else(|| RenderError::InvalidProject("subtitle start overflow".into()))?;
                let end = offset
                    .checked_add(get_u64(cue, "end_tick")?)
                    .ok_or_else(|| RenderError::InvalidProject("subtitle end overflow".into()))?;
                let start = start.max(range_start);
                let end = end.min(range_end);
                if start < end {
                    let text = required_string(cue, "text")?;
                    cues.push((start - range_start, end - range_start, text.to_owned()));
                }
            }
        }
    }
    cues.sort_by_key(|cue| (cue.0, cue.1));
    if cues.is_empty() {
        return Ok(None);
    }
    let mut document = String::from("\u{feff}");
    for (index, (start, end, text)) in cues.iter().enumerate() {
        if index != 0 {
            document.push_str("\r\n");
        }
        document.push_str(&(index + 1).to_string());
        document.push_str("\r\n");
        document.push_str(&format!(
            "{} --> {}\r\n",
            ticks_to_srt(*start),
            ticks_to_srt(*end)
        ));
        document.push_str(text);
        document.push_str("\r\n");
    }
    Ok(Some((document, "srt")))
}

/// ASS/SSA burn: the active document is exported through the subtitle core
/// (applying offset and cue edits while preserving styles). Event timing stays
/// absolute so move/fade/karaoke clocks survive range previews. Returns `None` when no styled
/// document is active so the SRT path can run.
fn prepare_styled_subtitles(project: &ProjectSnapshot) -> Result<Option<String>, RenderError> {
    let active_tracks: HashSet<_> = project
        .tracks
        .iter()
        .filter(|t| t.kind == "subtitle" && t.enabled)
        .map(|t| t.id.as_str())
        .collect();
    let active: Vec<&Value> = project
        .subtitles
        .iter()
        .filter(|sub| {
            sub.get("track_id")
                .and_then(Value::as_str)
                .is_some_and(|id| active_tracks.contains(id))
        })
        .collect();
    let styled = active.iter().any(|sub| {
        matches!(
            sub.get("format").and_then(Value::as_str),
            Some("ass" | "ssa")
        )
    });
    if !styled {
        return Ok(None);
    }
    if active.len() != 1 {
        return Err(RenderError::UnsupportedFeature(
            "burning an ASS/SSA document together with other subtitle documents is unsupported; merge them into one styled document".into(),
        ));
    }
    let subtitle: storycut_core::Subtitle = serde_json::from_value(active[0].clone())
        .map_err(|error| RenderError::InvalidProject(format!("subtitle document: {error}")))?;
    let export =
        storycut_subtitle::export_subtitle(&subtitle, subtitle.format).map_err(|error| {
            RenderError::UnsupportedFeature(format!("styled subtitle burn: {error}"))
        })?;
    Ok(Some(export.content))
}

fn run_render(
    project: &ProjectSnapshot,
    video_inputs: &[(InputSpec, usize, usize, usize, Motion, Option<u64>)],
    audio_inputs: &[(InputSpec, usize, Track, AudioSettings, u64, u64)],
    subtitle_path: Option<&Path>,
    output: &Path,
    range_start: u64,
    duration_ticks: u64,
    frame_count: u64,
    sample_count: u64,
    subtitle_mode: &str,
    loudness: Option<&LoudnessPlan>,
) -> Result<(), RenderError> {
    let total_seconds = ticks_to_seconds(duration_ticks);
    let mut args: Vec<String> = vec![
        "-hide_banner".into(),
        "-nostdin".into(),
        "-loglevel".into(),
        "error".into(),
        "-y".into(),
    ];
    let mut input_map = HashMap::new();
    let mut index = 0_usize;
    for (spec, ..) in video_inputs {
        if spec.loop_image {
            args.extend([
                "-loop".into(),
                "1".into(),
                "-framerate".into(),
                fps_string(project).into(),
                "-t".into(),
                ticks_to_seconds(spec.duration),
                "-threads:v".into(),
                "1".into(),
                "-i".into(),
                spec.path.to_string_lossy().into_owned(),
            ]);
        } else {
            args.extend([
                "-ss".into(),
                ticks_to_seconds(spec.source_in),
                "-t".into(),
                ticks_to_seconds(spec.duration),
                "-threads:v".into(),
                "1".into(),
                "-i".into(),
                spec.path.to_string_lossy().into_owned(),
            ]);
        }
        input_map.insert(spec.clip_id.clone(), index);
        index += 1;
    }
    for (spec, ..) in audio_inputs {
        args.extend([
            "-ss".into(),
            ticks_to_seconds(spec.source_in),
            "-t".into(),
            ticks_to_seconds(spec.duration),
            "-threads:a".into(),
            "1".into(),
            "-i".into(),
            spec.path.to_string_lossy().into_owned(),
        ]);
        input_map.insert(spec.clip_id.clone(), index);
        index += 1;
    }

    let mut graph = Vec::new();
    let color = format!("0x{}", &project.background[1..]);
    graph.push(format!(
        "color=c={color}:s={}x{}:r={}/{}:d={total_seconds},format=rgba[base0]",
        project.width, project.height, project.fps_num, project.fps_den
    ));
    let mut base_label = "base0".to_owned();
    for (ordinal, (spec, stream_ordinal, src_w, src_h, motion, fade_ticks)) in
        video_inputs.iter().enumerate()
    {
        let idx = *input_map
            .get(&spec.clip_id)
            .ok_or_else(|| RenderError::InvalidProject("video input mapping lost".into()))?;
        let clip = project
            .clips
            .iter()
            .find(|c| c.id == spec.clip_id)
            .ok_or_else(|| RenderError::InvalidProject("render clip disappeared".into()))?;
        let clip_timeline_start = clip.start.max(range_start) - range_start;
        let domain_tick_at_start = motion
            .sample_offset
            .checked_add(clip.start.max(range_start) - clip.start)
            .ok_or_else(|| RenderError::InvalidProject("motion sample offset overflow".into()))?;
        let timeline_start_seconds = ticks_to_seconds(clip_timeline_start);
        let base_scale = if motion.scale_is_static() {
            motion.first().scale
        } else {
            1.0
        };
        let (scaled_w, scaled_h) = fitted_dimensions(
            *src_w as f64,
            *src_h as f64,
            project.width,
            project.height,
            motion,
            base_scale,
        )?;
        let clip_end = clip
            .start
            .checked_add(clip.duration)
            .ok_or_else(|| RenderError::InvalidProject("clip end overflow".into()))?;
        let clip_timeline_end =
            clip_end.min(range_start.checked_add(duration_ticks).unwrap_or(u64::MAX)) - range_start;
        let start_seconds = timeline_start_seconds.clone();
        let end_seconds = ticks_to_seconds(clip_timeline_end);
        let x_motion = motion_expression(
            motion,
            MotionComponent::X,
            domain_tick_at_start,
            &timeline_start_seconds,
            "t",
        );
        let y_motion = motion_expression(
            motion,
            MotionComponent::Y,
            domain_tick_at_start,
            &timeline_start_seconds,
            "t",
        );
        let x_expr = format!(
            "({}-overlay_w)*{:.9}+({x_motion})*{}",
            project.width, motion.anchor_x, project.width
        );
        let y_expr = format!(
            "({}-overlay_h)*{:.9}+({y_motion})*{}",
            project.height, motion.anchor_y, project.height
        );
        let transform = if let Some(hold) = spec.hold {
            // Rebuild the held window at the project rate first, then place it.
            format!(
                "[{idx}:v:{stream_ordinal}]trim=duration={},setpts=PTS-STARTPTS,fps={}/{},tpad=start_mode=clone:start_duration={}:stop_mode=clone:stop_duration={},trim=duration={},setpts=PTS-STARTPTS+{start_seconds}/TB,scale={scaled_w}:{scaled_h}:flags=lanczos,format=rgba",
                ticks_to_seconds(spec.duration),
                project.fps_num,
                project.fps_den,
                ticks_to_seconds(hold.start_pad),
                ticks_to_seconds(hold.window),
                ticks_to_seconds(hold.window),
            )
        } else {
            format!(
                "[{idx}:v:{stream_ordinal}]trim=duration={},setpts=PTS-STARTPTS+{start_seconds}/TB,fps={}/{},scale={scaled_w}:{scaled_h}:flags=lanczos,format=rgba",
                ticks_to_seconds(spec.duration),
                project.fps_num,
                project.fps_den
            )
        };
        graph.push(format!("{transform}[layer{ordinal}raw]"));
        let mut layer_label = format!("layer{ordinal}raw");
        if !motion.scale_is_static() {
            let scale_motion = motion_expression(
                motion,
                MotionComponent::Scale,
                domain_tick_at_start,
                &timeline_start_seconds,
                "t",
            );
            let next = format!("layer{ordinal}scale");
            graph.push(format!(
                "[{layer_label}]scale=w='iw*({scale_motion})':h='ih*({scale_motion})':eval=frame:flags=lanczos[{next}]"
            ));
            layer_label = next;
        }
        if !motion.opacity_is_static() {
            let opacity_motion = motion_expression(
                motion,
                MotionComponent::Opacity,
                domain_tick_at_start,
                &timeline_start_seconds,
                "T",
            );
            let next = format!("layer{ordinal}opacity");
            graph.push(format!(
                "[{layer_label}]geq=r='r(X,Y)':g='g(X,Y)':b='b(X,Y)':a='alpha(X,Y)*({opacity_motion})'[{next}]"
            ));
            layer_label = next;
        } else if (motion.first().opacity - 1.0).abs() > f64::EPSILON {
            let next = format!("layer{ordinal}opacity");
            graph.push(format!(
                "[{layer_label}]colorchannelmixer=aa={:.9}[{next}]",
                motion.first().opacity
            ));
            layer_label = next;
        }
        if let Some(fade) = fade_ticks {
            if *fade > 0 && clip.start >= range_start {
                let fade_start = clip_timeline_start;
                let fade_duration = ticks_to_seconds(*fade);
                let fade_start = ticks_to_seconds(fade_start);
                graph.push(format!("[{layer_label}]fade=t=in:st={fade_start}:d={fade_duration}:alpha=1[layer{ordinal}fade]"));
                layer_label = format!("layer{ordinal}fade");
            }
        }
        let next = format!("base{}", ordinal + 1);
        graph.push(format!(
            "[{base_label}][{layer_label}]overlay=x='{x_expr}':y='{y_expr}':eval=frame:enable='between(t,{start_seconds},{end_seconds})':eof_action=pass:shortest=0:format=auto[{next}]"
        ));
        base_label = next;
    }

    let video_label = if let Some(subtitle_path) = subtitle_path {
        let file = subtitle_path
            .file_name()
            .and_then(OsStr::to_str)
            .ok_or_else(|| {
                RenderError::InvalidOptions("temporary subtitle filename is not UTF-8".into())
            })?;
        let escaped = file
            .replace('\\', "\\\\")
            .replace(':', "\\:")
            .replace('\'', "\\'");
        if subtitle_path.extension() == Some(OsStr::new("ass")) {
            // Keep styled events on their original clock. Clipping Dialogue
            // start/end would restart relative move, fade and karaoke effects.
            graph.push(format!(
                "[{base_label}]setpts=PTS+{}/TB,subtitles=filename='{escaped}',setpts=PTS-STARTPTS[vout]",
                ticks_to_seconds(range_start)
            ));
        } else {
            graph.push(format!(
                "[{base_label}]subtitles=filename='{escaped}'[vout]"
            ));
        }
        "vout"
    } else {
        base_label.as_str()
    };

    push_audio_graph(&mut graph, project, audio_inputs, &input_map, sample_count)?;
    let audio_out = if let Some(loudness) = loudness {
        graph.push(format!(
            "[aout]{},aresample={}:async=0,apad=whole_len={sample_count},atrim=end_sample={sample_count}[aoutn]",
            loudness.filter(), project.audio_sample_rate
        ));
        "[aoutn]"
    } else {
        "[aout]"
    };
    let (filter_script_path, mut filter_script) =
        create_temporary_filter_script(output.parent().unwrap_or_else(|| Path::new(".")))?;
    let _filter_script_guard = TempArtifact(filter_script_path.clone());
    filter_script.write_all(graph.join(";").as_bytes())?;
    drop(filter_script);
    let filter_script_name = filter_script_path
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| {
            RenderError::InvalidOptions("temporary filter graph filename is not UTF-8".into())
        })?;

    args.extend([
        "-filter_complex_script".into(),
        filter_script_name.to_owned(),
        "-map".into(),
        format!("[{video_label}]"),
        "-map".into(),
        audio_out.into(),
        "-frames:v".into(),
        frame_count.to_string(),
        "-c:v".into(),
        "libx264".into(),
        "-preset".into(),
        "medium".into(),
        "-crf".into(),
        "18".into(),
        "-pix_fmt".into(),
        "yuv420p".into(),
        "-r".into(),
        fps_string(project).into(),
        "-c:a".into(),
        "aac".into(),
        "-b:a".into(),
        "192k".into(),
        "-ar".into(),
        project.audio_sample_rate.to_string(),
        "-movflags".into(),
        "+faststart".into(),
        output.to_string_lossy().into_owned(),
    ]);
    let result = Command::new("ffmpeg")
        .current_dir(output.parent().unwrap_or_else(|| Path::new(".")))
        .args(&args)
        .output()?;
    if !result.status.success() {
        return Err(process_error("ffmpeg", result));
    }
    let _ = subtitle_mode;
    Ok(())
}

/// Appends the audio mix (per-clip envelopes, optional track ducking) ending in
/// `[aout]`. Shared by the render pass and the loudness measurement pass.
fn push_audio_graph(
    graph: &mut Vec<String>,
    project: &ProjectSnapshot,
    audio_inputs: &[(InputSpec, usize, Track, AudioSettings, u64, u64)],
    input_map: &HashMap<String, usize>,
    sample_count: u64,
) -> Result<(), RenderError> {
    let mut audio_labels = Vec::new();
    let mut audio_label_tracks: Vec<Track> = Vec::new();
    for (ordinal, (spec, stream_ordinal, track, settings, start_sample, end_sample)) in
        audio_inputs.iter().enumerate()
    {
        let idx = *input_map
            .get(&spec.clip_id)
            .ok_or_else(|| RenderError::InvalidProject("audio input mapping lost".into()))?;
        let length = end_sample.saturating_sub(*start_sample);
        if length == 0 {
            continue;
        }
        let total_domain = settings.domain_duration;
        let gain = db_to_linear(settings.gain_db + track.gain_db);
        let offset = ticks_to_seconds(settings.sample_offset);
        let fade_in = ticks_to_seconds(settings.fade_in);
        let total_domain_seconds = ticks_to_seconds(total_domain);
        let fade_out = ticks_to_seconds(settings.fade_out);
        let pan_left = (1.0 - settings.pan).clamp(0.0, 2.0);
        let pan_right = (1.0 + settings.pan).clamp(0.0, 2.0);
        let mut volume_expr = format!("{gain:.12}");
        if settings.fade_in > 0 {
            volume_expr.push_str(&format!("*min(1,max(0,(t+{offset})/{fade_in}))"));
        }
        if settings.fade_out > 0 {
            volume_expr.push_str(&format!(
                "*min(1,max(0,({total_domain_seconds}-t-{offset})/{fade_out}))"
            ));
        }
        let delayed = *start_sample;
        let label = format!("audio{ordinal}");
        graph.push(format!(
            "[{idx}:a:{stream_ordinal}]atrim=duration={},asetpts=PTS-STARTPTS,aresample={}:async=0,aformat=channel_layouts=stereo,pan=stereo|c0={pan_left:.9}*c0|c1={pan_right:.9}*c1,volume='{volume_expr}':eval=frame,adelay={delayed}S:all=1,apad=whole_len={sample_count},atrim=end_sample={sample_count}[{label}]",
            ticks_to_seconds(spec.duration), project.audio_sample_rate
        ));
        audio_labels.push(format!("[{label}]"));
        audio_label_tracks.push(track.clone());
    }
    let audio_labels = apply_track_ducking(graph, audio_labels, &audio_label_tracks, sample_count);
    if audio_labels.is_empty() {
        graph.push(format!(
            "anullsrc=r={}:cl=stereo,atrim=end_sample={sample_count}[aout]",
            project.audio_sample_rate
        ));
    } else {
        graph.push(format!(
            "{}amix=inputs={}:duration=longest:dropout_transition=0:normalize=0,atrim=end_sample={sample_count}[aout]",
            audio_labels.join(""), audio_labels.len()
        ));
    }
    Ok(())
}

/// First loudness pass: renders only the audio mix and reads `loudnorm`'s
/// JSON measurement. The video graph is not decoded.
fn measure_loudness(
    project: &ProjectSnapshot,
    audio_inputs: &[(InputSpec, usize, Track, AudioSettings, u64, u64)],
    sample_count: u64,
    target: LoudnessTarget,
    work_dir: &Path,
) -> Result<LoudnessPlan, RenderError> {
    if audio_inputs.is_empty() {
        return Err(RenderError::UnsupportedFeature(
            "master_loudness needs at least one audible audio clip in the render range".into(),
        ));
    }
    let mut args: Vec<String> = vec![
        "-hide_banner".into(),
        "-nostdin".into(),
        "-nostats".into(),
        "-loglevel".into(),
        "info".into(),
    ];
    let mut input_map = HashMap::new();
    for (index, (spec, ..)) in audio_inputs.iter().enumerate() {
        args.extend([
            "-ss".into(),
            ticks_to_seconds(spec.source_in),
            "-t".into(),
            ticks_to_seconds(spec.duration),
            "-threads:a".into(),
            "1".into(),
            "-i".into(),
            spec.path.to_string_lossy().into_owned(),
        ]);
        input_map.insert(spec.clip_id.clone(), index);
    }
    let mut graph = Vec::new();
    push_audio_graph(&mut graph, project, audio_inputs, &input_map, sample_count)?;
    // ebur128 is FFmpeg's reference meter; loudnorm's in-graph `input_i`
    // disagreed with it by >2 LU on ducked narration mixes.
    graph.push("[aout]ebur128=peak=true:framelog=quiet[ameasure]".to_owned());
    let (script_path, mut script) = create_temporary_filter_script(work_dir)?;
    let _guard = TempArtifact(script_path.clone());
    script.write_all(graph.join(";").as_bytes())?;
    drop(script);
    args.extend([
        "-filter_complex_script".into(),
        script_path.to_string_lossy().into_owned(),
        "-map".into(),
        "[ameasure]".into(),
        "-f".into(),
        "null".into(),
        "-".into(),
    ]);
    let result = Command::new("ffmpeg").args(&args).output()?;
    if !result.status.success() {
        return Err(process_error("ffmpeg loudness measurement", result));
    }
    let stderr = String::from_utf8_lossy(&result.stderr);
    let summary = stderr
        .rfind("Summary:")
        .map(|index| &stderr[index..])
        .ok_or_else(|| RenderError::VerificationFailed("ebur128 summary was not printed".into()))?;
    let value_after = |label: &str| -> Result<f64, RenderError> {
        summary
            .lines()
            .map(str::trim)
            .find_map(|line| line.strip_prefix(label))
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|value| value.is_finite())
            .ok_or_else(|| {
                RenderError::VerificationFailed(format!(
                    "ebur128 {label} is missing or not finite (silent mix?)"
                ))
            })
    };
    let measured_i = value_after("I:")?;
    Ok(LoudnessPlan {
        target,
        measured_i,
        measured_tp: value_after("Peak:")?,
        measured_lra: value_after("LRA:")?,
        measured_thresh: value_after("Threshold:")?,
        offset: target.integrated_lufs - measured_i,
    })
}

/// Groups clip signals into per-track buses when any audible track is ducked
/// and inserts `sidechaincompress` keyed by the source track's bus. Without
/// ducking the clip labels are returned unchanged, preserving the plain linear
/// mix contract.
fn apply_track_ducking(
    graph: &mut Vec<String>,
    labels: Vec<String>,
    tracks: &[Track],
    sample_count: u64,
) -> Vec<String> {
    let mut order: Vec<&str> = Vec::new();
    let mut members: HashMap<&str, Vec<&str>> = HashMap::new();
    for (label, track) in labels.iter().zip(tracks) {
        if !members.contains_key(track.id.as_str()) {
            order.push(track.id.as_str());
        }
        members
            .entry(track.id.as_str())
            .or_default()
            .push(label.as_str());
    }
    let track_by_id: HashMap<&str, &Track> = tracks.iter().map(|t| (t.id.as_str(), t)).collect();
    let ducked: Vec<(&str, &Ducking)> = order
        .iter()
        .filter_map(|id| {
            track_by_id[id]
                .ducking
                .as_ref()
                .filter(|d| members.contains_key(d.source_track_id.as_str()))
                .map(|d| (*id, d))
        })
        .collect();
    if ducked.is_empty() {
        return labels;
    }
    let mut sidechain_uses: HashMap<&str, usize> = HashMap::new();
    for (_, ducking) in &ducked {
        *sidechain_uses
            .entry(ducking.source_track_id.as_str())
            .or_default() += 1;
    }
    let mut bus_label: HashMap<&str, String> = HashMap::new();
    let mut sidechain_labels: HashMap<&str, Vec<String>> = HashMap::new();
    for (bus_index, id) in order.iter().enumerate() {
        let inputs = &members[id];
        let bus = format!("bus{bus_index}");
        let mixed = if inputs.len() == 1 {
            format!("{}anull", inputs[0])
        } else {
            format!(
                "{}amix=inputs={}:duration=longest:dropout_transition=0:normalize=0",
                inputs.join(""),
                inputs.len()
            )
        };
        if let Some(uses) = sidechain_uses.get(id) {
            let mut outs = vec![format!("[{bus}]")];
            let mut sidechains = Vec::new();
            for use_index in 0..*uses {
                let label = format!("{bus}sc{use_index}");
                outs.push(format!("[{label}]"));
                sidechains.push(format!("[{label}]"));
            }
            graph.push(format!(
                "{mixed},atrim=end_sample={sample_count},asplit={}{}",
                uses + 1,
                outs.join("")
            ));
            sidechain_labels.insert(id, sidechains);
        } else {
            graph.push(format!("{mixed},atrim=end_sample={sample_count}[{bus}]"));
        }
        bus_label.insert(id, format!("[{bus}]"));
    }
    for (bus_index, (id, ducking)) in ducked.iter().enumerate() {
        let sidechain = sidechain_labels
            .get_mut(ducking.source_track_id.as_str())
            .and_then(Vec::pop)
            .expect("sidechain split planned for every ducked source use");
        let out = format!("duck{bus_index}");
        graph.push(format!(
            "{}{sidechain}sidechaincompress=threshold={:.9}:ratio={:.6}:attack={:.6}:release={:.6}:makeup=1,atrim=end_sample={sample_count}[{out}]",
            bus_label[id], ducking.threshold, ducking.ratio, ducking.attack_ms, ducking.release_ms
        ));
        bus_label.insert(id, format!("[{out}]"));
    }
    order.iter().map(|id| bus_label[id].clone()).collect()
}

#[derive(Clone, Copy)]
enum MotionComponent {
    X,
    Y,
    Scale,
    Opacity,
}

fn motion_component_value(key: MotionKeyframe, component: MotionComponent) -> f64 {
    match component {
        MotionComponent::X => key.x,
        MotionComponent::Y => key.y,
        MotionComponent::Scale => key.scale,
        MotionComponent::Opacity => key.opacity,
    }
}

fn motion_expression(
    motion: &Motion,
    component: MotionComponent,
    domain_tick_at_start: u64,
    timeline_start_seconds: &str,
    time_variable: &str,
) -> String {
    let first = motion.first();
    let value = motion_component_value(first, component);
    if motion.is_static() {
        return format!("{value:.12}");
    }
    let Some(last) = motion.keyframes.get(1).copied() else {
        return format!("{value:.12}");
    };
    let last_value = motion_component_value(last, component);
    if (last_value - value).abs() <= f64::EPSILON {
        return format!("{value:.12}");
    }
    let domain_seconds = ticks_to_seconds(domain_tick_at_start);
    let first_seconds = ticks_to_seconds(first.tick);
    let interval_seconds = ticks_to_seconds(last.tick - first.tick);
    let raw_progress = format!(
        "(({domain_seconds}+({time_variable}-{timeline_start_seconds})-{first_seconds})/{interval_seconds})"
    );
    let progress = format!("min(max({raw_progress},0),1)");
    let eased = if motion.interpolation == "smoothstep" {
        format!("(({progress})*({progress})*(3-2*({progress})))")
    } else {
        progress
    };
    format!("({value:.12}+({:.12})*({eased}))", last_value - value)
}

fn fitted_dimensions(
    src_w: f64,
    src_h: f64,
    width: u32,
    height: u32,
    motion: &Motion,
    motion_scale: f64,
) -> Result<(u32, u32), RenderError> {
    let sx = width as f64 / src_w;
    let sy = height as f64 / src_h;
    let fit = if motion.fit == "cover" {
        sx.max(sy)
    } else {
        sx.min(sy)
    };
    let scale = fit * motion_scale;
    let w = (src_w * scale).round().max(1.0) as u32;
    let h = (src_h * scale).round().max(1.0) as u32;
    Ok((w, h))
}

fn validate_motion_edges(
    motion: &Motion,
    src_w: f64,
    src_h: f64,
    width: u32,
    height: u32,
) -> Result<(), RenderError> {
    if !motion.avoid_exposed_edges {
        return Ok(());
    }
    let fit_x = width as f64 / src_w;
    let fit_y = height as f64 / src_h;
    let fit = if motion.fit == "cover" {
        fit_x.max(fit_y)
    } else {
        fit_x.min(fit_y)
    };
    let dynamic_scale = !motion.scale_is_static();
    let base_width = (src_w * fit).round().max(1.0);
    let base_height = (src_h * fit).round().max(1.0);
    for key in &motion.keyframes {
        let (w, h) = if dynamic_scale {
            (
                (base_width * key.scale).round().max(1.0),
                (base_height * key.scale).round().max(1.0),
            )
        } else {
            (
                (src_w * fit * key.scale).round().max(1.0),
                (src_h * fit * key.scale).round().max(1.0),
            )
        };
        let x = (width as f64 - w) * motion.anchor_x + key.x * width as f64;
        let y = (height as f64 - h) * motion.anchor_y + key.y * height as f64;
        if x > 0.0 || y > 0.0 || x + w < width as f64 || y + h < height as f64 {
            return Err(RenderError::InvalidProject(
                "avoid_exposed_edges is true but the motion path leaves part of the canvas uncovered".into(),
            ));
        }
    }
    Ok(())
}

fn validate_layer_pixel_budget(
    motion: &Motion,
    src_w: u32,
    src_h: u32,
    width: u32,
    height: u32,
    clip_id: &str,
) -> Result<(), RenderError> {
    let source_pixels = u128::from(src_w) * u128::from(src_h);
    let fit_x = width as f64 / src_w as f64;
    let fit_y = height as f64 / src_h as f64;
    let fit = if motion.fit == "cover" {
        fit_x.max(fit_y)
    } else {
        fit_x.min(fit_y)
    };
    let max_scale = motion
        .keyframes
        .iter()
        .map(|key| key.scale)
        .fold(0.0_f64, f64::max);
    let (layer_width, layer_height) = if motion.scale_is_static() {
        (
            (src_w as f64 * fit * max_scale).round().max(1.0) as u64,
            (src_h as f64 * fit * max_scale).round().max(1.0) as u64,
        )
    } else {
        let base_width = (src_w as f64 * fit).round().max(1.0);
        let base_height = (src_h as f64 * fit).round().max(1.0);
        (
            (base_width * max_scale).round().max(1.0) as u64,
            (base_height * max_scale).round().max(1.0) as u64,
        )
    };
    let layer_pixels = u128::from(layer_width) * u128::from(layer_height);
    let required_pixels = source_pixels.max(layer_pixels);
    if required_pixels > MAX_LAYER_PIXELS {
        return Err(RenderError::UnsupportedFeature(format!(
            "clip {clip_id} source is {src_w}x{src_h} ({source_pixels} pixels) and transforms to a {layer_width}x{layer_height} layer ({layer_pixels} pixels); the per-layer render limit is {MAX_LAYER_PIXELS} pixels"
        )));
    }
    Ok(())
}

#[derive(Debug)]
struct OutputFacts {
    streams: Vec<String>,
    audio_samples: u64,
    duration_ticks: u64,
}

fn verify_output(
    path: &Path,
    project: &ProjectSnapshot,
    expected_frames: u64,
    expected_samples: u64,
    expected_duration_ticks: u64,
) -> Result<OutputFacts, RenderError> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-count_frames",
            "-show_streams",
            "-show_format",
            "-of",
            "json",
        ])
        .arg(path)
        .output()?;
    if !output.status.success() {
        return Err(process_error("ffprobe", output));
    }
    let data: Value = serde_json::from_slice(&output.stdout)?;
    let streams = data
        .get("streams")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            RenderError::VerificationFailed("ffprobe output has no streams array".into())
        })?;
    let video = streams
        .iter()
        .find(|s| s.get("codec_type").and_then(Value::as_str) == Some("video"))
        .ok_or_else(|| RenderError::VerificationFailed("output has no video stream".into()))?;
    let audio = streams
        .iter()
        .find(|s| s.get("codec_type").and_then(Value::as_str) == Some("audio"))
        .ok_or_else(|| RenderError::VerificationFailed("output has no audio stream".into()))?;
    let width = get_u32_value(video, "width")?;
    let height = get_u32_value(video, "height")?;
    if width != project.width || height != project.height {
        return Err(RenderError::VerificationFailed(format!(
            "video dimensions are {width}x{height}, expected {}x{}",
            project.width, project.height
        )));
    }
    let actual_frames = video
        .get("nb_read_frames")
        .and_then(Value::as_str)
        .and_then(|s| s.parse::<u64>().ok())
        .or_else(|| video.get("nb_read_frames").and_then(Value::as_u64))
        .ok_or_else(|| {
            RenderError::VerificationFailed(
                "ffprobe did not report decoded video frame count".into(),
            )
        })?;
    if actual_frames != expected_frames {
        return Err(RenderError::VerificationFailed(format!(
            "decoded frame count {actual_frames}, expected {expected_frames}"
        )));
    }
    let video_decode = Command::new("ffmpeg")
        .args(["-hide_banner", "-nostdin", "-v", "error", "-xerror", "-i"])
        .arg(path)
        .args(["-map", "0:v:0", "-f", "null", "-"])
        .output()?;
    if !video_decode.status.success() {
        return Err(process_error("ffmpeg video decode verifier", video_decode));
    }
    let sample_rate = audio
        .get("sample_rate")
        .and_then(Value::as_str)
        .and_then(|s| s.parse::<u32>().ok())
        .or_else(|| {
            audio
                .get("sample_rate")
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok())
        })
        .ok_or_else(|| RenderError::VerificationFailed("audio sample rate is missing".into()))?;
    if sample_rate != project.audio_sample_rate {
        return Err(RenderError::VerificationFailed(format!(
            "audio sample rate {sample_rate}, expected {}",
            project.audio_sample_rate
        )));
    }
    // AAC packet padding means container duration is not a valid sample count. Decode to PCM and count it.
    let actual_samples = count_decoded_audio_samples(path, project.audio_sample_rate)?;
    // AAC encoder delay/padding may leave up to one AAC frame of decoded samples beyond the trim.
    if actual_samples.abs_diff(expected_samples) > 1024 {
        return Err(RenderError::VerificationFailed(format!(
            "decoded audio sample count {actual_samples} differs from planned {expected_samples} by more than AAC frame padding"
        )));
    }
    let actual_duration_ticks = data
        .pointer("/format/duration")
        .and_then(Value::as_str)
        .and_then(decimal_seconds_to_ticks)
        .ok_or_else(|| {
            RenderError::VerificationFailed("ffprobe did not report container duration".into())
        })?;
    if actual_duration_ticks.abs_diff(expected_duration_ticks) > project.frame_ticks {
        return Err(RenderError::VerificationFailed(format!(
            "container duration {actual_duration_ticks} ticks differs from planned {expected_duration_ticks} by more than one frame"
        )));
    }
    let mut result = Vec::new();
    for stream in streams {
        let kind = stream
            .get("codec_type")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let codec = stream
            .get("codec_name")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        result.push(format!("{kind}:{codec}"));
    }
    Ok(OutputFacts {
        streams: result,
        audio_samples: actual_samples,
        duration_ticks: actual_duration_ticks,
    })
}

fn count_decoded_audio_samples(path: &Path, sample_rate: u32) -> Result<u64, RenderError> {
    let mut child = Command::new("ffmpeg")
        .args(["-hide_banner", "-nostdin", "-v", "error", "-xerror", "-i"])
        .arg(path)
        .args(["-map", "0:a:0", "-f", "s16le", "-ac", "2", "-ar"])
        .arg(sample_rate.to_string())
        .arg("-")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| RenderError::VerificationFailed("audio verifier has no stdout".into()))?;
    let mut buffer = [0_u8; 64 * 1024];
    let mut bytes = 0_u64;
    loop {
        match stdout.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => {
                bytes = bytes.checked_add(count as u64).ok_or_else(|| {
                    RenderError::VerificationFailed("decoded audio byte count overflow".into())
                })?
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(RenderError::Io(error));
            }
        }
    }
    let status = child.wait()?;
    if !status.success() {
        return Err(RenderError::ProcessFailed {
            program: "ffmpeg audio decode verifier".into(),
            detail: format!("exit status {status}"),
        });
    }
    if bytes % 4 != 0 {
        return Err(RenderError::VerificationFailed(
            "decoded stereo PCM ended on a partial sample frame".into(),
        ));
    }
    Ok(bytes / 4)
}

fn probe_facts(path: &Path) -> Result<MediaFacts, RenderError> {
    if !path.is_file() {
        return Err(RenderError::InvalidProject(format!(
            "media file does not exist: {}",
            path.display()
        )));
    }
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-count_frames",
            "-show_streams",
            "-show_format",
            "-of",
            "json",
        ])
        .arg(path)
        .output()?;
    if !output.status.success() {
        return Err(process_error("ffprobe", output));
    }
    let json: Value = serde_json::from_slice(&output.stdout)?;
    let streams_json = json
        .get("streams")
        .and_then(Value::as_array)
        .ok_or_else(|| RenderError::ProcessFailed {
            program: "ffprobe".into(),
            detail: "missing streams".into(),
        })?;
    if streams_json.is_empty() {
        return Err(RenderError::InvalidProject(format!(
            "media has no streams: {}",
            path.display()
        )));
    }
    let mut streams = Vec::new();
    let mut public_streams = Vec::new();
    let mut per_kind = BTreeMap::<String, u32>::new();
    let mut width = None;
    let mut height = None;
    for stream in streams_json {
        let kind = stream
            .get("codec_type")
            .and_then(Value::as_str)
            .unwrap_or("");
        if kind != "video" && kind != "audio" {
            continue;
        }
        let index = get_u32_value(stream, "index")?;
        let ordinal = *per_kind.entry(kind.to_owned()).or_insert(0);
        *per_kind.get_mut(kind).expect("entry inserted") += 1;
        let time_base_value = stream
            .get("time_base")
            .and_then(Value::as_str)
            .ok_or_else(|| RenderError::ProcessFailed {
                program: "ffprobe".into(),
                detail: format!("stream {index} has no time_base"),
            })?;
        let time_base = parse_ratio(time_base_value).ok_or_else(|| {
            RenderError::InvalidProject(format!("invalid stream time_base {time_base_value}"))
        })?;
        let sample_rate = stream
            .get("sample_rate")
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<u32>().ok())
            .or_else(|| {
                stream
                    .get("sample_rate")
                    .and_then(Value::as_u64)
                    .and_then(|n| u32::try_from(n).ok())
            });
        let stream_width = stream
            .get("width")
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok());
        let stream_height = stream
            .get("height")
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok());
        if kind == "video" && width.is_none() {
            width = stream_width;
            height = stream_height;
        }
        let frames = stream
            .get("nb_read_frames")
            .and_then(Value::as_str)
            .and_then(|s| s.parse().ok())
            .or_else(|| stream.get("nb_read_frames").and_then(Value::as_u64));
        streams.push(StreamFacts {
            index,
            kind: kind.into(),
            ordinal,
            width: stream_width,
            height: stream_height,
            frames,
        });
        public_streams.push(ProbeStream {
            index,
            kind: kind.into(),
            time_base: StreamTimeBase {
                num: time_base.0,
                den: time_base.1,
            },
            sample_rate,
        });
    }
    if public_streams.is_empty() {
        return Err(RenderError::InvalidProject(format!(
            "media has no audio/video streams: {}",
            path.display()
        )));
    }
    let format = json.get("format").and_then(Value::as_object);
    let format_name = format
        .and_then(|f| f.get("format_name"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let probed_duration_ticks = format
        .and_then(|f| f.get("duration"))
        .and_then(Value::as_str)
        .and_then(decimal_seconds_to_ticks)
        .or_else(|| {
            streams_json
                .iter()
                .filter_map(|s| {
                    s.get("duration")
                        .and_then(Value::as_str)
                        .and_then(decimal_seconds_to_ticks)
                })
                .max()
        });
    let ext = path
        .extension()
        .and_then(OsStr::to_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    let has_video = streams.iter().any(|s| s.kind == "video");
    let has_audio = streams.iter().any(|s| s.kind == "audio");
    let kind = if ["png", "jpg", "jpeg", "webp"].contains(&ext.as_str()) && has_video {
        let frames = streams
            .iter()
            .filter(|s| s.kind == "video")
            .filter_map(|s| s.frames)
            .max()
            .unwrap_or(1);
        if frames > 1 {
            return Err(RenderError::UnsupportedFeature(format!(
                "animated image input is not supported: {}",
                path.display()
            )));
        }
        AssetKind::Image
    } else if has_video {
        AssetKind::Video
    } else if has_audio {
        AssetKind::Audio
    } else {
        return Err(RenderError::InvalidProject(format!(
            "unsupported media file: {}",
            path.display()
        )));
    };
    // Still images have no media timeline duration; image2's nominal 25 fps
    // packet duration is an FFmpeg demuxer default, not a clip-length bound.
    let duration_ticks = if matches!(kind, AssetKind::Image) {
        None
    } else {
        probed_duration_ticks
    };
    let report = ProbeReport {
        kind,
        duration_ticks,
        sha256: sha256_file(path)?,
        streams: public_streams,
        width,
        height,
        format_name,
    };
    Ok(MediaFacts { report, streams })
}

fn probe_kind_compatible(declared: &str, actual: &AssetKind) -> bool {
    matches!(
        (declared, actual),
        ("image", AssetKind::Image) | ("video", AssetKind::Video) | ("audio", AssetKind::Audio)
    )
}

fn process_error(program: &str, output: Output) -> RenderError {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    RenderError::ProcessFailed {
        program: program.into(),
        detail: if stderr.is_empty() {
            format!("exit status {}", output.status)
        } else {
            stderr
        },
    }
}

fn tool_version(program: &str) -> Result<String, RenderError> {
    let output = Command::new(program)
        .arg("-version")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()?;
    if !output.status.success() {
        return Err(process_error(program, output));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap_or("unknown")
        .to_owned())
}

fn create_temporary_output(
    parent: &Path,
    output: &Path,
) -> Result<(PathBuf, Option<File>), RenderError> {
    let stem = output
        .file_stem()
        .and_then(OsStr::to_str)
        .unwrap_or("render");
    for _ in 0..100 {
        let serial = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(
            ".{stem}.storycut-{}-{serial}.tmp.mp4",
            std::process::id()
        ));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, Some(file))),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(RenderError::Io(error)),
        }
    }
    Err(RenderError::InvalidOptions(
        "could not allocate a unique temporary render path".into(),
    ))
}

fn create_temporary_filter_script(parent: &Path) -> Result<(PathBuf, File), RenderError> {
    for _ in 0..100 {
        let serial = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(
            ".storycut-{}-{serial}.filtergraph",
            std::process::id()
        ));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(RenderError::Io(error)),
        }
    }
    Err(RenderError::InvalidOptions(
        "could not allocate a unique temporary filter graph".into(),
    ))
}

fn unique_candidate(parent: &Path, stem: &str, extension: &str) -> Result<PathBuf, RenderError> {
    for _ in 0..100 {
        let serial = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(
            ".{stem}.storycut-{}-{serial}.{extension}",
            std::process::id()
        ));
        if !path.exists() {
            return Ok(path);
        }
    }
    Err(RenderError::InvalidOptions(
        "could not allocate a unique temporary path".into(),
    ))
}

fn write_sidecar(
    parent: &Path,
    output: &Path,
    contents: &str,
    extension: &str,
) -> Result<PathBuf, RenderError> {
    write_sidecar_with(parent, output, contents, extension, |file, text| {
        file.write_all(text.as_bytes())
    })
}

fn write_sidecar_with<F>(
    parent: &Path,
    output: &Path,
    contents: &str,
    extension: &str,
    mut write_contents: F,
) -> Result<PathBuf, RenderError>
where
    F: FnMut(&mut File, &str) -> std::io::Result<()>,
{
    let stem = output
        .file_stem()
        .and_then(OsStr::to_str)
        .unwrap_or("render");
    for _ in 0..100 {
        let serial = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(
            ".{stem}.storycut-{}-{serial}.{extension}",
            std::process::id()
        ));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                let result = write_contents(&mut file, contents).and_then(|()| file.sync_all());
                drop(file);
                if let Err(error) = result {
                    // The sidecar has not been handed to FFmpeg yet. Remove any
                    // partially written contents before returning the failure.
                    let _ = fs::remove_file(&path);
                    return Err(RenderError::Io(error));
                }
                return Ok(path);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(RenderError::Io(error)),
        }
    }
    Err(RenderError::InvalidOptions(
        "could not allocate subtitle sidecar".into(),
    ))
}

fn project_base_dir(project_path: &Path) -> Result<PathBuf, RenderError> {
    let path = absolute_path(project_path)?;
    let base = if path.is_dir() {
        path
    } else {
        path.parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf()
    };
    canonicalize_allow_missing_tail(&base).map_err(RenderError::Io)
}

fn canonical_media_root(
    project_parent: &Path,
    authorized_root: Option<&Path>,
) -> Result<PathBuf, RenderError> {
    let root = match authorized_root {
        Some(root) => absolute_path(root)?,
        // project_base_dir canonicalized all existing components. Keep any
        // missing suffix lexical so direct callers may render from an in-memory
        // project snapshot before its JSON file or parent has been created.
        None => return Ok(project_parent.to_path_buf()),
    };
    let canonical = fs::canonicalize(root)?;
    if !canonical.is_dir() {
        return Err(RenderError::InvalidOptions(format!(
            "authorized media root is not a directory: {}",
            canonical.display()
        )));
    }
    Ok(canonical)
}

fn canonicalize_allow_missing_tail(path: &Path) -> std::io::Result<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() && !normalized.has_root() {
                    normalized.push(component.as_os_str());
                }
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }

    let mut ancestor = normalized;
    let mut missing = Vec::new();
    loop {
        match fs::canonicalize(&ancestor) {
            Ok(mut canonical) => {
                for component in missing.iter().rev() {
                    canonical.push(component);
                }
                return Ok(canonical);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let Some(name) = ancestor.file_name() else {
                    return Err(error);
                };
                missing.push(name.to_os_string());
                if !ancestor.pop() {
                    return Err(error);
                }
            }
            Err(error) => return Err(error),
        }
    }
}

fn resolve_source(
    base: &Path,
    source: &Path,
    authorized_root: &Path,
) -> Result<PathBuf, RenderError> {
    let joined = if source.is_absolute() {
        source.to_path_buf()
    } else {
        base.join(source)
    };
    let resolved = fs::canonicalize(joined)?;
    if !resolved.starts_with(authorized_root) {
        return Err(RenderError::InvalidProject(format!(
            "asset source resolves outside the authorized media root: {}",
            resolved.display()
        )));
    }
    Ok(resolved)
}

fn publish_without_overwrite(source: &Path, output: &Path) -> Result<(), RenderError> {
    publish_without_overwrite_with(source, output, |path| fs::remove_file(path))
}

fn publish_without_overwrite_with<F>(
    source: &Path,
    output: &Path,
    cleanup_temporary: F,
) -> Result<(), RenderError>
where
    F: FnOnce(&Path) -> std::io::Result<()>,
{
    match fs::hard_link(source, output) {
        Ok(()) => {
            // The hard link is the commit point. Cleanup is best-effort: once
            // the destination exists with the verified bytes, failure to remove
            // the temporary alias must not report the render as failed.
            let _ = cleanup_temporary(source);
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let _ = cleanup_temporary(source);
            Err(RenderError::OutputExists(output.to_path_buf()))
        }
        Err(error) => {
            let _ = cleanup_temporary(source);
            Err(RenderError::Io(error))
        }
    }
}

fn absolute_path(path: &Path) -> Result<PathBuf, RenderError> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

fn sha256_file(path: &Path) -> Result<String, RenderError> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn ticks_to_seconds(ticks: u64) -> String {
    format!("{:.9}", ticks as f64 / TIMEBASE as f64)
}

fn fps_string(project: &ProjectSnapshot) -> String {
    format!("{}/{}", project.fps_num, project.fps_den)
}

fn ticks_to_samples(ticks: u64, sample_rate: u32) -> Result<u64, RenderError> {
    let numerator = u128::from(ticks) * u128::from(sample_rate);
    let denominator = u128::from(TIMEBASE);
    let whole = numerator / denominator;
    let remainder = numerator % denominator;
    let rounded = if remainder * 2 > denominator || (remainder * 2 == denominator && whole % 2 == 1)
    {
        whole + 1
    } else {
        whole
    };
    u64::try_from(rounded)
        .map_err(|_| RenderError::InvalidProject("audio sample count overflow".into()))
}

fn div_ceil(value: u64, divisor: u64) -> u64 {
    value / divisor + u64::from(value % divisor != 0)
}

fn db_to_linear(db: f64) -> f64 {
    10_f64.powf(db / 20.0)
}

fn decimal_seconds_to_ticks(value: &str) -> Option<u64> {
    let value = value.trim();
    let (negative, value) = value
        .strip_prefix('-')
        .map_or((false, value), |v| (true, v));
    if negative {
        return None;
    }
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    if whole.is_empty()
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || !fraction.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let whole: u128 = whole.parse().ok()?;
    let decimal_den = 10_u128.checked_pow(fraction.len().try_into().ok()?)?;
    let frac: u128 = if fraction.is_empty() {
        0
    } else {
        fraction.parse().ok()?
    };
    let numerator = whole.checked_mul(decimal_den)?.checked_add(frac)?;
    let scaled = numerator.checked_mul(u128::from(TIMEBASE))?;
    let q = scaled / decimal_den;
    let r = scaled % decimal_den;
    let rounded = if r * 2 > decimal_den || (r * 2 == decimal_den && q % 2 == 1) {
        q + 1
    } else {
        q
    };
    u64::try_from(rounded).ok()
}

fn parse_ratio(value: &str) -> Option<(u32, u32)> {
    let (num, den) = value.split_once('/')?;
    let num: u32 = num.parse().ok()?;
    let den: u32 = den.parse().ok()?;
    if num == 0 || den == 0 || num > 1_000_000 || den > 1_000_000 {
        return None;
    }
    Some((num, den))
}

fn ticks_to_srt(ticks: u64) -> String {
    let ms = ticks_to_millis(ticks);
    let hours = ms / 3_600_000;
    let minutes = (ms / 60_000) % 60;
    let seconds = (ms / 1_000) % 60;
    let millis = ms % 1_000;
    format!("{hours:02}:{minutes:02}:{seconds:02},{millis:03}")
}

fn ticks_to_millis(ticks: u64) -> u64 {
    let den = TIMEBASE / 1_000;
    let q = ticks / den;
    let r = ticks % den;
    if r * 2 > den || (r * 2 == den && q % 2 == 1) {
        q + 1
    } else {
        q
    }
}

fn object<'a>(value: &'a Value, name: &str) -> Result<&'a Map<String, Value>, RenderError> {
    value
        .as_object()
        .ok_or_else(|| RenderError::InvalidProject(format!("{name} must be an object")))
}

fn array<'a>(object: &'a Map<String, Value>, name: &str) -> Result<&'a Vec<Value>, RenderError> {
    object
        .get(name)
        .and_then(Value::as_array)
        .ok_or_else(|| RenderError::InvalidProject(format!("{name} must be an array")))
}

fn required_string<'a>(object: &'a Map<String, Value>, name: &str) -> Result<&'a str, RenderError> {
    object
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| RenderError::InvalidProject(format!("{name} must be a string")))
}

fn get_u64(object: &Map<String, Value>, name: &str) -> Result<u64, RenderError> {
    object
        .get(name)
        .and_then(Value::as_u64)
        .ok_or_else(|| RenderError::InvalidProject(format!("{name} must be an unsigned integer")))
}

fn get_u32(object: &Map<String, Value>, name: &str) -> Result<u32, RenderError> {
    let value = get_u64(object, name)?;
    u32::try_from(value)
        .map_err(|_| RenderError::InvalidProject(format!("{name} exceeds u32 range")))
}

fn optional_u32(object: &Map<String, Value>, name: &str) -> Result<Option<u32>, RenderError> {
    object
        .get(name)
        .filter(|v| !v.is_null())
        .map(|v| {
            let n = v.as_u64().ok_or_else(|| {
                RenderError::InvalidProject(format!("{name} must be an unsigned integer"))
            })?;
            u32::try_from(n)
                .map_err(|_| RenderError::InvalidProject(format!("{name} exceeds u32 range")))
        })
        .transpose()
}

fn optional_u64(value: &Value, name: &str) -> Result<Option<u64>, RenderError> {
    value
        .get(name)
        .filter(|v| !v.is_null())
        .map(|v| {
            v.as_u64().ok_or_else(|| {
                RenderError::InvalidOptions(format!("{name} must be an unsigned integer"))
            })
        })
        .transpose()
}

fn number(object: &Map<String, Value>, name: &str) -> Result<Option<f64>, RenderError> {
    object
        .get(name)
        .filter(|v| !v.is_null())
        .map(|v| {
            let n = v
                .as_f64()
                .ok_or_else(|| RenderError::InvalidProject(format!("{name} must be a number")))?;
            if !n.is_finite() {
                return Err(RenderError::InvalidProject(format!(
                    "{name} must be finite"
                )));
            }
            Ok(n)
        })
        .transpose()
}

fn validate_finite_range(name: &str, value: f64, min: f64, max: f64) -> Result<(), RenderError> {
    if !value.is_finite() || value < min || value > max {
        return Err(RenderError::InvalidProject(format!(
            "{name} must be between {min} and {max}"
        )));
    }
    Ok(())
}

fn get_u32_value(value: &Value, name: &str) -> Result<u32, RenderError> {
    let n = value
        .get(name)
        .and_then(Value::as_u64)
        .ok_or_else(|| RenderError::VerificationFailed(format!("missing {name}")))?;
    u32::try_from(n)
        .map_err(|_| RenderError::VerificationFailed(format!("{name} exceeds u32 range")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_sample_conversion_uses_absolute_nearest_ties_even() {
        assert_eq!(ticks_to_samples(TIMEBASE / 48_000, 48_000).unwrap(), 1);
        assert_eq!(ticks_to_samples(TIMEBASE / 96_000, 48_000).unwrap(), 0);
        assert_eq!(ticks_to_samples(3 * TIMEBASE / 96_000, 48_000).unwrap(), 2);
    }

    #[test]
    fn probe_duration_decimal_is_rounded_without_float_seconds() {
        assert_eq!(
            decimal_seconds_to_ticks("1.5"),
            Some(TIMEBASE + TIMEBASE / 2)
        );
        assert_eq!(decimal_seconds_to_ticks("0.000000001"), Some(1));
        assert_eq!(decimal_seconds_to_ticks("nan"), None);
    }

    #[test]
    fn srt_timestamps_are_rounded_to_nearest_millisecond() {
        assert_eq!(ticks_to_srt(TIMEBASE), "00:00:01,000");
        assert_eq!(
            ticks_to_srt(TIMEBASE * 3_661 + TIMEBASE / 2),
            "01:01:01,500"
        );
    }

    #[test]
    fn animated_motion_keyframes_are_parsed_for_domain_sampling() {
        let frame_ticks = TIMEBASE / 8;
        let domain_duration = 2 * TIMEBASE;
        let motion = serde_json::json!({
            "domain_duration_ticks":domain_duration,
            "sample_offset_tick":0,
            "fit":"cover", "avoid_exposed_edges":false,
            "anchor":{"x":0.5,"y":0.5}, "interpolation":"linear",
            "keyframes":[
                {"tick":0,"x":0.0,"y":0.0,"scale":1.0,"opacity":1.0},
                {"tick":domain_duration-frame_ticks,"x":0.1,"y":0.0,"scale":1.0,"opacity":1.0}
            ]
        });
        let parsed = parse_motion(&motion, frame_ticks, domain_duration).unwrap();
        assert!(!parsed.is_static());
        assert_eq!(parsed.keyframes[0].x, 0.0);
        assert_eq!(parsed.keyframes[1].x, 0.1);
    }

    #[test]
    fn avoid_exposed_edges_checks_the_whole_animated_path() {
        let frame_ticks = TIMEBASE / 8;
        let domain_duration = TIMEBASE;
        let motion = serde_json::json!({
            "domain_duration_ticks":domain_duration,
            "sample_offset_tick":0,
            "fit":"cover", "avoid_exposed_edges":true,
            "anchor":{"x":0.5,"y":0.5}, "interpolation":"linear",
            "keyframes":[
                {"tick":0,"x":-0.01,"y":0.0,"scale":1.0,"opacity":1.0},
                {"tick":domain_duration-frame_ticks,"x":0.01,"y":0.0,"scale":1.0,"opacity":1.0}
            ]
        });
        let parsed = parse_motion(&motion, frame_ticks, domain_duration).unwrap();
        assert_eq!(
            validate_motion_edges(&parsed, 160.0, 90.0, 160, 90)
                .unwrap_err()
                .code(),
            "INVALID_PROJECT"
        );

        let covered = serde_json::json!({
            "domain_duration_ticks":domain_duration,
            "sample_offset_tick":0,
            "fit":"cover", "avoid_exposed_edges":true,
            "anchor":{"x":0.5,"y":0.5}, "interpolation":"linear",
            "keyframes":[
                {"tick":0,"x":-0.05,"y":-0.05,"scale":1.1,"opacity":1.0},
                {"tick":domain_duration-frame_ticks,"x":0.05,"y":0.05,"scale":1.18,"opacity":1.0}
            ]
        });
        let parsed = parse_motion(&covered, frame_ticks, domain_duration).unwrap();
        validate_motion_edges(&parsed, 160.0, 90.0, 160, 90)
            .expect("cover plus the full pan range should keep every edge filled");
    }

    #[test]
    fn layer_pixel_budget_accepts_limit_and_rejects_static_and_animated_blowups() {
        let frame = TIMEBASE / 8;
        let make_motion = |start_scale: f64, end_scale: f64| Motion {
            fit: "cover".into(),
            anchor_x: 0.5,
            anchor_y: 0.5,
            avoid_exposed_edges: false,
            sample_offset: 0,
            interpolation: "linear".into(),
            keyframes: vec![
                MotionKeyframe {
                    tick: 0,
                    x: 0.0,
                    y: 0.0,
                    scale: start_scale,
                    opacity: 1.0,
                },
                MotionKeyframe {
                    tick: TIMEBASE - frame,
                    x: 0.0,
                    y: 0.0,
                    scale: end_scale,
                    opacity: 1.0,
                },
            ],
        };

        validate_layer_pixel_budget(&make_motion(1.0, 1.0), 4096, 4096, 4096, 4096, "at-limit")
            .expect("exactly 16,777,216 decoded/transformed pixels should be allowed");
        let just_over = validate_layer_pixel_budget(
            &make_motion(1.0, 1.0),
            4098,
            4096,
            4098,
            4096,
            "source-over-limit",
        )
        .unwrap_err();
        assert_eq!(just_over.code(), "UNSUPPORTED_FEATURE");

        let static_zoom = validate_layer_pixel_budget(
            &make_motion(16.0, 16.0),
            1920,
            1080,
            1920,
            1080,
            "static-zoom",
        )
        .unwrap_err();
        assert_eq!(static_zoom.code(), "UNSUPPORTED_FEATURE");

        let animated_zoom = validate_layer_pixel_budget(
            &make_motion(1.0, 16.0),
            1920,
            1080,
            1920,
            1080,
            "animated-zoom",
        )
        .unwrap_err();
        assert_eq!(animated_zoom.code(), "UNSUPPORTED_FEATURE");
        assert!(animated_zoom.to_string().contains("per-layer render limit"));
    }

    #[test]
    fn render_options_never_allow_overwrite() {
        let options = serde_json::json!({"overwrite":true});
        assert_eq!(
            parse_options(&options).unwrap_err().code(),
            "INVALID_OPTIONS"
        );
    }

    #[test]
    fn published_output_succeeds_when_temporary_cleanup_fails() {
        let directory = tempfile::tempdir().unwrap();
        let temporary = directory.path().join("verified.tmp");
        let output = directory.path().join("final.mp4");
        fs::write(&temporary, b"verified render bytes").unwrap();

        publish_without_overwrite_with(&temporary, &output, |_| {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "simulated cleanup failure",
            ))
        })
        .expect("successful hard-link publication is the commit point");

        assert_eq!(fs::read(&output).unwrap(), b"verified render bytes");
        assert!(
            temporary.exists(),
            "simulated cleanup leaves only the temp alias"
        );
    }

    #[test]
    fn failed_sidecar_write_removes_partial_file() {
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("render.mp4");

        let error = write_sidecar_with(
            directory.path(),
            &output,
            "full subtitle",
            "srt",
            |file, _| {
                file.write_all(b"partial subtitle")?;
                Err(std::io::Error::new(
                    std::io::ErrorKind::WriteZero,
                    "simulated write failure",
                ))
            },
        )
        .unwrap_err();

        assert_eq!(error.code(), "IO_ERROR");
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[test]
    fn source_paths_must_resolve_under_the_project_media_root() {
        let directory = tempfile::tempdir().unwrap();
        let project_root = directory.path().join("project");
        fs::create_dir(&project_root).unwrap();
        fs::create_dir(project_root.join("media")).unwrap();
        fs::write(project_root.join("media/source.bin"), b"inside").unwrap();
        fs::write(directory.path().join("outside.bin"), b"outside").unwrap();
        let canonical_root = fs::canonicalize(&project_root).unwrap();

        assert_eq!(
            resolve_source(
                &canonical_root,
                Path::new("media/source.bin"),
                &canonical_root
            )
            .unwrap(),
            fs::canonicalize(project_root.join("media/source.bin")).unwrap()
        );
        let error = resolve_source(
            &canonical_root,
            Path::new("../outside.bin"),
            &canonical_root,
        )
        .unwrap_err();
        assert_eq!(error.code(), "INVALID_PROJECT");
        assert!(
            error
                .to_string()
                .contains("outside the authorized media root")
        );
    }

    #[test]
    fn explicit_media_root_can_contain_sources_outside_project_parent() {
        let directory = tempfile::tempdir().unwrap();
        let project_root = directory.path().join("workspace/project");
        let media_root = directory.path().join("workspace");
        fs::create_dir_all(&project_root).unwrap();
        fs::write(media_root.join("shared.bin"), b"workspace media").unwrap();
        let canonical_project_root = fs::canonicalize(&project_root).unwrap();
        let canonical_media_root = fs::canonicalize(&media_root).unwrap();

        assert_eq!(
            resolve_source(
                &canonical_project_root,
                Path::new("../shared.bin"),
                &canonical_media_root
            )
            .unwrap(),
            fs::canonicalize(media_root.join("shared.bin")).unwrap()
        );
    }
}

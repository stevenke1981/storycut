//! `storycut_narration_assemble`: narration-driven story assembly for the
//! Night Lantern profile (docs/NIGHT_LANTERN_PROFILE.md).
//!
//! Narration segments are laid end to end on their own track; every shot
//! covers a contiguous range of segments, so picture length follows the voice.
//! Centered dissolves keep the total length: each clip is extended by half a
//! window past its cut, and native videos get frozen holds of one full window
//! so their continuous core is never blended.

use super::{
    CommandError, FocalMotionConfig, command_to_core, compute_focal_motion, core_error,
    default_motion, field_bool, field_str, field_u64, finite_number, floor_to_grid, generated_id,
    make_track, parse_focal_motion_config, require_object_keys, round_to_grid,
    seconds_value_to_ticks, semantic_identity, success, with_projection_warning,
};
use serde_json::{Value, json};
use std::path::Path;
use storycut_core::{
    Anchor, Asset, AssetKind, AudioClip, AudioSettings, Clip, CoreError, Ducking, FitMode,
    ImageClip, Interpolation, Keyframe, Motion, MAX_TICKS, Operation, Project, ProjectStore,
    StreamKind, TIMEBASE, TrackKind, Transition, TransitionKind, VideoAudioPolicy, VideoClip,
};

const MAX_SEGMENTS: usize = 400;
const MAX_SHOTS: usize = 300;

pub(crate) fn narration_assemble(workspace: &Path, args: &Value) -> Result<Value, CommandError> {
    require_object_keys(
        args,
        &[
            "project_id",
            "expected_revision",
            "idempotency_key",
            "dry_run",
            "narration",
            "intro",
            "shots",
            "outro",
            "transition",
            "music",
            "overlays",
        ],
    )?;
    let project_id = field_str(args, "project_id")?;
    let expected_revision = field_u64(args, "expected_revision")?;
    let idempotency_key = field_str(args, "idempotency_key")?;
    let dry_run = field_bool(args, "dry_run")?;
    let store = ProjectStore::load(workspace, project_id).map_err(core_error)?;
    let planned = store
        .apply_planned(
            expected_revision,
            idempotency_key,
            dry_run,
            "storycut_narration_assemble",
            semantic_identity(args),
            |project| plan(workspace, project, args, idempotency_key),
        )
        .map_err(core_error)?;
    let mut data = planned.data.as_object().cloned().unwrap_or_default();
    data.insert("committed".into(), json!(planned.result.applied));
    data.insert("base_revision".into(), json!(planned.result.base_revision));
    data.insert("changed_ids".into(), json!(planned.result.changed_ids));
    data.insert(
        "duration_ticks".into(),
        json!(planned.result.duration_ticks),
    );
    let mut response = success(
        Some(project_id),
        Some(planned.result.revision),
        Value::Object(data),
    );
    if let Some(warnings) = planned.data.get("warnings").and_then(Value::as_array) {
        if !warnings.is_empty() {
            response["warnings"] = json!(warnings);
        }
    }
    Ok(with_projection_warning(
        response,
        planned.result.projection_warning.as_deref(),
    ))
}

fn fail(message: impl Into<String>) -> CoreError {
    CoreError::validation(message.into())
}

fn find_asset<'a>(project: &'a Project, id: &str) -> Result<&'a Asset, CoreError> {
    project
        .assets
        .iter()
        .find(|asset| asset.id == id)
        .ok_or_else(|| fail(format!("asset {id} was not found")))
}

fn stream_index(asset: &Asset, kind: StreamKind) -> Result<u64, CoreError> {
    asset
        .streams
        .iter()
        .find(|stream| stream.kind == kind)
        .map(|stream| stream.index)
        .ok_or_else(|| fail(format!("asset {} has no {kind:?} stream", asset.id)))
}

fn optional_seconds(
    value: Option<&Value>,
    field: &str,
    allow_zero: bool,
) -> Result<Option<u64>, CoreError> {
    value
        .filter(|value| !value.is_null())
        .map(|value| seconds_value_to_ticks(value, field, allow_zero))
        .transpose()
}

fn gain(value: Option<&Value>, field: &str) -> Result<f64, CoreError> {
    let gain = finite_number(value, 0.0, field)?;
    if !(-96.0..=24.0).contains(&gain) {
        return Err(fail(format!("{field} must be within -96..=24 dB")));
    }
    Ok(gain)
}

fn audio_settings(duration: u64, gain_db: f64, fade_in: u64, fade_out: u64) -> AudioSettings {
    AudioSettings {
        domain_duration_ticks: duration,
        sample_offset_tick: 0,
        gain_db,
        pan: 0.0,
        muted: false,
        fade_in_ticks: fade_in,
        fade_out_ticks: fade_out,
        fade_curve: "linear_amplitude".into(),
    }
}

/// One picture on the story track, before cuts are resolved.
struct Visual {
    role: &'static str,
    asset_id: String,
    kind: AssetKind,
    /// Video: frames read from the source (the native core). Image: unused.
    core: u64,
    source_in: u64,
    cut_in: u64,
    cut_out: u64,
    motion: Option<FocalMotionConfig>,
}

fn visual_from(
    project: &Project,
    item: &Value,
    frame: u64,
    role: &'static str,
    context: &str,
) -> Result<(Visual, Option<u64>), CoreError> {
    let asset_id = item
        .get("asset_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| fail(format!("{context}.asset_id is required")))?;
    let asset = find_asset(project, asset_id)?;
    let explicit_duration = optional_seconds(
        item.get("duration_seconds"),
        &format!("{context}.duration_seconds"),
        false,
    )?
    .map(|ticks| round_to_grid(ticks, frame))
    .transpose()?;
    match asset.kind {
        AssetKind::Image => {
            if item.get("source_in_seconds").is_some() {
                return Err(fail(format!("{context}: images have no source_in_seconds")));
            }
            Ok((
                Visual {
                    role,
                    asset_id: asset_id.to_owned(),
                    kind: AssetKind::Image,
                    core: 0,
                    source_in: 0,
                    cut_in: 0,
                    cut_out: 0,
                    motion: None,
                },
                explicit_duration,
            ))
        }
        AssetKind::Video => {
            stream_index(asset, StreamKind::Video)?;
            let natural = asset
                .duration_ticks
                .ok_or_else(|| fail(format!("video asset {asset_id} has no probed duration")))?;
            let source_in = optional_seconds(
                item.get("source_in_seconds"),
                &format!("{context}.source_in_seconds"),
                true,
            )?
            .map(|ticks| round_to_grid(ticks, frame))
            .transpose()?
            .unwrap_or(0);
            let available = floor_to_grid(natural.saturating_sub(source_in), frame)?;
            let core = explicit_duration.unwrap_or(available);
            if core == 0 || core > available {
                return Err(fail(format!(
                    "{context}: video {asset_id} source range is empty or exceeds its duration"
                )));
            }
            Ok((
                Visual {
                    role,
                    asset_id: asset_id.to_owned(),
                    kind: AssetKind::Video,
                    core,
                    source_in,
                    cut_in: 0,
                    cut_out: 0,
                    motion: None,
                },
                None,
            ))
        }
        AssetKind::Audio => Err(fail(format!("{context}: asset {asset_id} is not visual"))),
    }
}

fn build_overlay_motion(
    duration_ticks: u64,
    frame_ticks: u64,
    fade_in_ticks: u64,
    fade_out_ticks: u64,
    scale: f64,
    x: f64,
    y: f64,
) -> Motion {
    let last_domain_tick = duration_ticks.saturating_sub(frame_ticks);
    let keyframes = if last_domain_tick == 0 {
        vec![Keyframe {
            tick: 0,
            x,
            y,
            scale,
            opacity: if fade_in_ticks > 0 { 0.0 } else { 1.0 },
        }]
    } else {
        let mut raw_keys = vec![(0, if fade_in_ticks > 0 { 0.0 } else { 1.0 })];
        if fade_in_ticks > 0 {
            let in_tick = fade_in_ticks.min(last_domain_tick);
            if in_tick > 0 && in_tick < last_domain_tick {
                raw_keys.push((in_tick, 1.0));
            }
        }
        if fade_out_ticks > 0 {
            let out_start = duration_ticks.saturating_sub(fade_out_ticks).min(last_domain_tick);
            if out_start > 0 && out_start < last_domain_tick {
                if let Some(last) = raw_keys.last() {
                    if out_start > last.0 {
                        raw_keys.push((out_start, 1.0));
                    }
                }
            }
        }
        if let Some(last) = raw_keys.last() {
            if last.0 < last_domain_tick {
                raw_keys.push((last_domain_tick, if fade_out_ticks > 0 { 0.0 } else { 1.0 }));
            }
        }
        raw_keys
            .into_iter()
            .map(|(tick, opacity)| Keyframe {
                tick,
                x,
                y,
                scale,
                opacity,
            })
            .collect()
    };

    Motion {
        domain_duration_ticks: duration_ticks,
        sample_offset_tick: 0,
        fit: FitMode::Cover,
        avoid_exposed_edges: false,
        anchor: Anchor { x: 0.5, y: 0.5 },
        interpolation: Interpolation::Linear,
        keyframes,
    }
}

#[allow(clippy::too_many_lines)]
fn plan(
    workspace: &Path,
    project: &Project,
    args: &Value,
    key: &str,
) -> Result<(Vec<Operation>, Value), CoreError> {
    let frame = project
        .frame_ticks()
        .ok_or_else(|| fail("project frame rate cannot be represented by the timebase"))?;
    let sample = TIMEBASE / u64::from(project.audio_sample_rate);
    if sample == 0 || TIMEBASE % u64::from(project.audio_sample_rate) != 0 || frame % sample != 0 {
        return Err(fail("project frame and sample grids must nest exactly"));
    }

    // Transition: centered dissolve window W (even number of frames), half h.
    let transition = args
        .get("transition")
        .cloned()
        .unwrap_or_else(|| json!({"mode":"cut"}));
    require_object_keys(&transition, &["mode", "duration_seconds"]).map_err(command_to_core)?;
    let window = match transition
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or("cut")
    {
        "cut" => 0,
        "centered_dissolve" => {
            let requested = seconds_value_to_ticks(
                transition.get("duration_seconds").unwrap_or(&json!(0.4)),
                "transition.duration_seconds",
                false,
            )?;
            let window = round_to_grid(requested, 2 * frame)?;
            if window == 0 {
                return Err(fail("centered dissolve rounds to zero frames"));
            }
            window
        }
        other => {
            return Err(fail(format!(
                "transition.mode {other} must be cut or centered_dissolve"
            )));
        }
    };
    let half = window / 2;

    let mut operations = Vec::new();
    let mut created_track_ids = Vec::new();

    // Visual track first, overlays later so they draw on top.
    let visual_track = generated_id(project, key, "narration-visual-track", 0);
    operations.push(Operation::TrackAdd {
        track: make_track(visual_track.clone(), "故事畫面", TrackKind::Video),
    });
    created_track_ids.push(visual_track.clone());

    // ---- Intro -------------------------------------------------------------
    let mut visuals: Vec<Visual> = Vec::new();
    let mut lead_in = 0_u64;
    let mut intro_title_overlay = None;
    if let Some(intro) = args.get("intro").filter(|v| !v.is_null()) {
        require_object_keys(
            intro,
            &[
                "asset_id",
                "source_in_seconds",
                "duration_seconds",
                "title_overlay_asset_id",
                "title_duration_seconds",
                "title_fade_in_seconds",
                "title_fade_out_seconds",
                "title_scale",
                "title_x",
                "title_y",
            ],
        )
        .map_err(command_to_core)?;
        let (mut visual, image_duration) = visual_from(project, intro, frame, "intro", "intro")?;
        let slot = match visual.kind {
            AssetKind::Image => {
                image_duration.ok_or_else(|| fail("intro image needs duration_seconds"))?
            }
            _ => visual.core + half,
        };
        visual.cut_in = 0;
        visual.cut_out = slot;
        lead_in = slot;
        visuals.push(visual);

        if let Some(title_asset_id) = intro.get("title_overlay_asset_id").and_then(Value::as_str) {
            if !title_asset_id.is_empty() {
                let asset = find_asset(project, title_asset_id)?;
                let title_duration = match intro.get("title_duration_seconds") {
                    Some(val) => seconds_value_to_ticks(val, "intro.title_duration_seconds", false)?,
                    None => match asset.kind {
                        AssetKind::Image => {
                            let default_ticks = (7.8 * TIMEBASE as f64).round() as u64;
                            default_ticks.min(slot)
                        }
                        AssetKind::Video => {
                            asset.duration_ticks.ok_or_else(|| {
                                fail(format!(
                                    "title overlay video {title_asset_id} has no probed duration"
                                ))
                            })?
                        }
                        AssetKind::Audio => {
                            return Err(fail(format!(
                                "title overlay asset {title_asset_id} is not visual"
                            )));
                        }
                    },
                };
                let title_duration = round_to_grid(title_duration, frame)?;
                if title_duration == 0 {
                    return Err(fail("intro.title_duration_seconds rounds to zero frames"));
                }
                let default_fade_in = (1.0 * TIMEBASE as f64).round() as u64;
                let default_fade_out = (1.0 * TIMEBASE as f64).round() as u64;
                let title_fade_in = match intro.get("title_fade_in_seconds") {
                    Some(val) => round_to_grid(
                        seconds_value_to_ticks(val, "intro.title_fade_in_seconds", true)?,
                        frame,
                    )?,
                    None => round_to_grid(default_fade_in.min(title_duration / 2), frame)?,
                };
                let title_fade_out = match intro.get("title_fade_out_seconds") {
                    Some(val) => round_to_grid(
                        seconds_value_to_ticks(val, "intro.title_fade_out_seconds", true)?,
                        frame,
                    )?,
                    None => round_to_grid(
                        default_fade_out.min(title_duration.saturating_sub(title_fade_in)),
                        frame,
                    )?,
                };
                if title_fade_in + title_fade_out > title_duration {
                    return Err(fail("intro title overlay fades exceed title duration"));
                }
                let scale = finite_number(intro.get("title_scale"), 1.0, "intro.title_scale")?;
                let x = finite_number(intro.get("title_x"), 0.0, "intro.title_x")?;
                let y = finite_number(intro.get("title_y"), 0.0, "intro.title_y")?;
                if !(-4.0..=4.0).contains(&x) {
                    return Err(fail("intro.title_x must be between -4.0 and 4.0"));
                }
                if !(-4.0..=4.0).contains(&y) {
                    return Err(fail("intro.title_y must be between -4.0 and 4.0"));
                }
                if !(1e-6..=16.0).contains(&scale) {
                    return Err(fail("intro.title_scale must be between 1e-6 and 16.0"));
                }
                intro_title_overlay = Some((
                    title_asset_id.to_owned(),
                    title_duration,
                    title_fade_in,
                    title_fade_out,
                    scale,
                    x,
                    y,
                ));
            }
        }
    }

    // ---- Narration ---------------------------------------------------------
    let narration = args
        .get("narration")
        .ok_or_else(|| fail("narration is required"))?;
    require_object_keys(
        narration,
        &["track_name", "gap_seconds", "gain_db", "segments"],
    )
    .map_err(command_to_core)?;
    let segments = narration
        .get("segments")
        .and_then(Value::as_array)
        .filter(|segments| !segments.is_empty() && segments.len() <= MAX_SEGMENTS)
        .ok_or_else(|| {
            fail(format!(
                "narration.segments must contain 1 to {MAX_SEGMENTS} items"
            ))
        })?;
    let gap = optional_seconds(narration.get("gap_seconds"), "narration.gap_seconds", true)?
        .map(|ticks| round_to_grid(ticks, sample))
        .transpose()?
        .unwrap_or(0);
    let lane_gain = gain(narration.get("gain_db"), "narration.gain_db")?;
    let narration_track = generated_id(project, key, "narration-voice-track", 0);
    let track_name = narration
        .get("track_name")
        .and_then(Value::as_str)
        .unwrap_or("旁白");
    operations.push(Operation::TrackAdd {
        track: make_track(narration_track.clone(), track_name, TrackKind::Audio),
    });
    created_track_ids.push(narration_track.clone());
    let mut cursor = lead_in;
    let mut segment_offsets = Vec::new();
    let mut starts = Vec::with_capacity(segments.len());
    for (index, segment) in segments.iter().enumerate() {
        require_object_keys(segment, &["asset_id", "gain_db"]).map_err(command_to_core)?;
        let asset_id = segment
            .get("asset_id")
            .and_then(Value::as_str)
            .ok_or_else(|| fail(format!("narration.segments[{index}].asset_id is required")))?;
        let asset = find_asset(project, asset_id)?;
        if asset.kind == AssetKind::Image {
            return Err(fail(format!("narration segment {asset_id} is not audio")));
        }
        let stream = stream_index(asset, StreamKind::Audio)?;
        let duration = floor_to_grid(
            asset
                .duration_ticks
                .ok_or_else(|| fail(format!("audio asset {asset_id} has no probed duration")))?,
            sample,
        )?;
        if duration == 0 {
            return Err(fail(format!("narration segment {asset_id} is empty")));
        }
        let clip_id = generated_id(project, key, "narration-voice-clip", index);
        let segment_gain = gain(segment.get("gain_db"), "segment gain_db")? + lane_gain;
        operations.push(Operation::ClipAdd {
            clip: Clip::Audio(AudioClip {
                id: clip_id.clone(),
                track_id: narration_track.clone(),
                asset_id: asset_id.to_owned(),
                start_tick: cursor,
                duration_ticks: duration,
                source_in_tick: 0,
                stream_index: stream,
                audio: audio_settings(duration, segment_gain.clamp(-96.0, 24.0), 0, 0),
            }),
        });
        starts.push(cursor);
        segment_offsets.push(json!({
            "index":index,"asset_id":asset_id,"clip_id":clip_id,
            "start_tick":cursor,"duration_ticks":duration,
            "start_seconds":cursor as f64 / TIMEBASE as f64
        }));
        cursor = cursor
            .checked_add(duration)
            .and_then(|end| {
                if index + 1 < segments.len() {
                    end.checked_add(gap)
                } else {
                    Some(end)
                }
            })
            .filter(|end| *end <= MAX_TICKS)
            .ok_or_else(|| fail("narration timeline overflows"))?;
    }
    let narration_end = cursor;
    // Frame boundaries of the segments; the last boundary ends the story body.
    let mut boundaries = Vec::with_capacity(starts.len() + 1);
    for start in &starts {
        boundaries.push(round_to_grid(*start, frame)?);
    }
    // Round the story end up so the last narration sample always has picture.
    boundaries.push(narration_end.div_ceil(frame) * frame);
    if boundaries.windows(2).any(|pair| pair[1] <= pair[0]) {
        return Err(fail("every narration segment must last at least one frame"));
    }

    // ---- Shots -------------------------------------------------------------
    let shots = args
        .get("shots")
        .and_then(Value::as_array)
        .filter(|shots| !shots.is_empty() && shots.len() <= MAX_SHOTS)
        .ok_or_else(|| fail(format!("shots must contain 1 to {MAX_SHOTS} items")))?;
    let first_shot = visuals.len();
    let mut expected_first = 0_usize;
    let mut aligns = Vec::new();
    for (index, shot) in shots.iter().enumerate() {
        let context = format!("shots[{index}]");
        require_object_keys(
            shot,
            &[
                "asset_id",
                "segments",
                "align",
                "source_in_seconds",
                "duration_seconds",
                "motion",
            ],
        )
        .map_err(command_to_core)?;
        let range = shot
            .get("segments")
            .and_then(Value::as_array)
            .filter(|range| range.len() == 2)
            .and_then(|range| Some((range[0].as_u64()? as usize, range[1].as_u64()? as usize)))
            .ok_or_else(|| {
                fail(format!(
                    "{context}.segments must be [first, last] segment indexes"
                ))
            })?;
        if range.0 != expected_first || range.1 < range.0 || range.1 >= segments.len() {
            return Err(fail(format!(
                "{context}.segments must continue at segment {expected_first} and stay within the narration"
            )));
        }
        expected_first = range.1 + 1;
        let (mut visual, image_duration) = visual_from(project, shot, frame, "shot", &context)?;
        if visual.kind == AssetKind::Image && image_duration.is_some() {
            return Err(fail(format!(
                "{context}: image length follows its segments; remove duration_seconds"
            )));
        }
        visual.cut_in = boundaries[range.0];
        visual.cut_out = boundaries[range.1 + 1];
        if let Some(motion_val) = shot.get("motion").filter(|v| !v.is_null()) {
            visual.motion = Some(parse_focal_motion_config(motion_val)?);
        }
        let align = shot
            .get("align")
            .and_then(Value::as_str)
            .unwrap_or("center");
        if !matches!(align, "start" | "center" | "end") {
            return Err(fail(format!(
                "{context}.align must be start, center or end"
            )));
        }
        if visual.kind == AssetKind::Image && shot.get("align").is_some() {
            return Err(fail(format!(
                "{context}: align applies to video shots only"
            )));
        }
        aligns.push(align);
        visuals.push(visual);
    }
    if expected_first != segments.len() {
        return Err(fail("shots must cover every narration segment in order"));
    }
    let last_shot = visuals.len() - 1;

    // ---- Outro -------------------------------------------------------------
    if let Some(outro) = args.get("outro").filter(|v| !v.is_null()) {
        require_object_keys(
            outro,
            &["asset_id", "source_in_seconds", "duration_seconds"],
        )
        .map_err(command_to_core)?;
        let (mut visual, image_duration) = visual_from(project, outro, frame, "outro", "outro")?;
        let slot = match visual.kind {
            AssetKind::Image => {
                image_duration.ok_or_else(|| fail("outro image needs duration_seconds"))?
            }
            _ => visual.core + half,
        };
        visual.cut_in = boundaries[boundaries.len() - 1];
        visual.cut_out = visual.cut_in + slot;
        visuals.push(visual);
    }
    let count = visuals.len();

    // Native videos inside the story body take a fixed slot; neighbours yield.
    let spans: Vec<(u64, u64)> = visuals.iter().map(|v| (v.cut_in, v.cut_out)).collect();
    let mut native_slots = Vec::new();
    for index in first_shot..=last_shot {
        if visuals[index].kind != AssetKind::Video {
            continue;
        }
        let dissolve_in = window > 0 && index > 0;
        let dissolve_out = window > 0 && index + 1 < count;
        let slot = visuals[index].core
            + if dissolve_in { half } else { 0 }
            + if dissolve_out { half } else { 0 };
        let (span_in, span_out) = spans[index];
        let cut_in = match aligns[index - first_shot] {
            "start" => span_in,
            "end" => span_out
                .checked_sub(slot)
                .ok_or_else(|| fail("native shot is longer than the timeline before it"))?,
            _ => {
                let span = span_out - span_in;
                if span >= slot {
                    span_in + floor_to_grid((span - slot) / 2, frame)?
                } else {
                    span_in
                        .checked_sub(floor_to_grid((slot - span) / 2, frame)?)
                        .ok_or_else(|| fail("native shot overruns the story start"))?
                }
            }
        };
        let cut_out = cut_in + slot;
        if index == first_shot && cut_in != span_in {
            return Err(fail(format!(
                "native shot {} is the first shot and must start with the narration (align: start)",
                visuals[index].asset_id
            )));
        }
        if index == last_shot && cut_out != span_out {
            return Err(fail(format!(
                "native shot {} is the last shot and must end with the narration (align: end)",
                visuals[index].asset_id
            )));
        }
        visuals[index].cut_in = cut_in;
        visuals[index].cut_out = cut_out;
        if index > 0 {
            visuals[index - 1].cut_out = cut_in;
        }
        if index + 1 < count {
            visuals[index + 1].cut_in = cut_out;
        }
        native_slots.push((index, slot));
    }
    for (index, slot) in native_slots {
        if visuals[index].cut_out.checked_sub(visuals[index].cut_in) != Some(slot) {
            return Err(fail(format!(
                "native shot {} collides with an adjacent native shot; give it more narration or change align",
                visuals[index].asset_id
            )));
        }
    }

    // Every picture needs visible frames of its own beyond the dissolves.
    for (index, visual) in visuals.iter().enumerate() {
        let min = if visual.kind == AssetKind::Video {
            1
        } else {
            window.max(frame)
        };
        if visual.cut_out <= visual.cut_in || visual.cut_out - visual.cut_in < min {
            return Err(fail(format!(
                "{} {} ({}) is too short after resolving native shots and dissolves",
                visual.role, index, visual.asset_id
            )));
        }
        if index > 0 && visuals[index - 1].cut_out != visual.cut_in {
            return Err(fail("internal: visual cuts are not contiguous"));
        }
    }

    // ---- Visual clips and transitions --------------------------------------
    let mut warnings = Vec::<Value>::new();
    let mut cut_report = Vec::new();
    let mut native_cores = Vec::new();
    let mut clip_ids = Vec::new();
    for (index, visual) in visuals.iter().enumerate() {
        let dissolve_in = window > 0 && index > 0;
        let dissolve_out = window > 0 && index + 1 < count;
        let start = visual.cut_in - if dissolve_in { half } else { 0 };
        let end = visual.cut_out + if dissolve_out { half } else { 0 };
        let duration = end - start;
        let clip_id = generated_id(project, key, "narration-visual-clip", index);
        let asset = find_asset(project, &visual.asset_id)?;
        let clip_motion = if let Some(cfg) = &visual.motion {
            compute_focal_motion(
                workspace,
                &project.canvas,
                asset,
                &clip_id,
                duration,
                frame,
                cfg,
                &mut warnings,
            )?
        } else {
            default_motion(duration, frame)
        };
        let clip = match visual.kind {
            AssetKind::Image => Clip::Image(ImageClip {
                id: clip_id.clone(),
                track_id: visual_track.clone(),
                asset_id: visual.asset_id.clone(),
                start_tick: start,
                duration_ticks: duration,
                source_in_tick: 0,
                motion: clip_motion,
            }),
            _ => {
                let head = if dissolve_in { window } else { 0 };
                let tail = if dissolve_out { window } else { 0 };
                if head + visual.core + tail != duration {
                    return Err(fail("internal: native slot does not match its holds"));
                }
                native_cores.push(json!({
                    "clip_id":clip_id,"asset_id":visual.asset_id,
                    "core_start_tick":start + head,"core_end_tick":start + head + visual.core
                }));
                Clip::Video(VideoClip {
                    id: clip_id.clone(),
                    track_id: visual_track.clone(),
                    asset_id: visual.asset_id.clone(),
                    start_tick: start,
                    duration_ticks: duration,
                    source_in_tick: visual.source_in,
                    stream_index: stream_index(asset, StreamKind::Video)?,
                    motion: clip_motion,
                    audio_policy: VideoAudioPolicy::Muted,
                    hold_head_ticks: head,
                    hold_tail_ticks: tail,
                })
            }
        };
        operations.push(Operation::ClipAdd { clip });
        if dissolve_in {
            operations.push(Operation::TransitionSet {
                transition: Transition {
                    id: generated_id(project, key, "narration-transition", index),
                    track_id: visual_track.clone(),
                    from_clip_id: clip_ids.last().cloned().expect("previous visual exists"),
                    to_clip_id: clip_id.clone(),
                    start_tick: start,
                    duration_ticks: window,
                    kind: TransitionKind::CrossDissolve,
                    curve: "linear".into(),
                    audio_policy: "independent".into(),
                },
            });
        }
        cut_report.push(json!({
            "role":visual.role,"clip_id":clip_id,"asset_id":visual.asset_id,
            "cut_in_tick":visual.cut_in,"cut_out_tick":visual.cut_out,
            "clip_start_tick":start,"clip_duration_ticks":duration
        }));
        clip_ids.push(clip_id);
    }
    let total = visuals
        .last()
        .map_or(narration_end, |visual| visual.cut_out)
        .max(narration_end);

    // ---- Music -------------------------------------------------------------
    let mut music_track = None;
    if let Some(music) = args.get("music").filter(|v| !v.is_null()) {
        require_object_keys(music, &["track_name", "ducking", "cues"]).map_err(command_to_core)?;
        let track_id = generated_id(project, key, "narration-music-track", 0);
        let mut track = make_track(
            track_id.clone(),
            music
                .get("track_name")
                .and_then(Value::as_str)
                .unwrap_or("配樂"),
            TrackKind::Audio,
        );
        track.ducking = match music.get("ducking") {
            Some(Value::Null) => None,
            None => Some(Ducking {
                source_track_id: narration_track.clone(),
                threshold: 0.015,
                ratio: 6.0,
                attack_ms: 30.0,
                release_ms: 500.0,
            }),
            Some(value) => {
                require_object_keys(value, &["threshold", "ratio", "attack_ms", "release_ms"])
                    .map_err(command_to_core)?;
                Some(Ducking {
                    source_track_id: narration_track.clone(),
                    threshold: finite_number(value.get("threshold"), 0.015, "ducking.threshold")?,
                    ratio: finite_number(value.get("ratio"), 6.0, "ducking.ratio")?,
                    attack_ms: finite_number(value.get("attack_ms"), 30.0, "ducking.attack_ms")?,
                    release_ms: finite_number(
                        value.get("release_ms"),
                        500.0,
                        "ducking.release_ms",
                    )?,
                })
            }
        };
        operations.push(Operation::TrackAdd { track });
        created_track_ids.push(track_id.clone());
        let cues = music
            .get("cues")
            .and_then(Value::as_array)
            .filter(|cues| !cues.is_empty() && cues.len() <= 64)
            .ok_or_else(|| fail("music.cues must contain 1 to 64 items"))?;
        let mut placed: Vec<(u64, u64)> = Vec::new();
        for (index, cue) in cues.iter().enumerate() {
            let context = format!("music.cues[{index}]");
            require_object_keys(
                cue,
                &[
                    "asset_id",
                    "start_seconds",
                    "end_seconds",
                    "source_in_seconds",
                    "gain_db",
                    "fade_in_seconds",
                    "fade_out_seconds",
                ],
            )
            .map_err(command_to_core)?;
            let asset_id = cue
                .get("asset_id")
                .and_then(Value::as_str)
                .ok_or_else(|| fail(format!("{context}.asset_id is required")))?;
            let asset = find_asset(project, asset_id)?;
            let stream = stream_index(asset, StreamKind::Audio)?;
            let start = round_to_grid(
                seconds_value_to_ticks(
                    cue.get("start_seconds").unwrap_or(&json!(0)),
                    "start_seconds",
                    true,
                )?,
                sample,
            )?;
            let end = match optional_seconds(cue.get("end_seconds"), "end_seconds", false)? {
                Some(ticks) => round_to_grid(ticks, sample)?.min(floor_to_grid(total, sample)?),
                None => floor_to_grid(total, sample)?,
            };
            if end <= start {
                return Err(fail(format!("{context} ends before it starts")));
            }
            let source_in =
                optional_seconds(cue.get("source_in_seconds"), "source_in_seconds", true)?
                    .map(|ticks| round_to_grid(ticks, sample))
                    .transpose()?
                    .unwrap_or(0);
            let natural = asset
                .duration_ticks
                .ok_or_else(|| fail(format!("music asset {asset_id} has no probed duration")))?;
            let duration = end - start;
            if source_in
                .checked_add(duration)
                .is_none_or(|source_end| source_end > natural)
            {
                return Err(fail(format!(
                    "{context}: {asset_id} is shorter than the cue; split it into several cues"
                )));
            }
            let fade = |name: &str| -> Result<u64, CoreError> {
                optional_seconds(cue.get(name), name, true)?
                    .map(|ticks| round_to_grid(ticks, sample))
                    .transpose()
                    .map(|value| value.unwrap_or(0))
            };
            let (fade_in, fade_out) = (fade("fade_in_seconds")?, fade("fade_out_seconds")?);
            if fade_in + fade_out > duration {
                return Err(fail(format!("{context} fades exceed the cue")));
            }
            placed.push((start, end));
            operations.push(Operation::ClipAdd {
                clip: Clip::Audio(AudioClip {
                    id: generated_id(project, key, "narration-music-clip", index),
                    track_id: track_id.clone(),
                    asset_id: asset_id.to_owned(),
                    start_tick: start,
                    duration_ticks: duration,
                    source_in_tick: source_in,
                    stream_index: stream,
                    audio: audio_settings(
                        duration,
                        gain(cue.get("gain_db"), "music gain_db")?,
                        fade_in,
                        fade_out,
                    ),
                }),
            });
        }
        music_track = Some(track_id);
    }

    // ---- Overlays (alpha cards, name plates, quotes) -----------------------
    let mut overlay_track = None;
    let has_overlays = args.get("overlays").is_some_and(|v| !v.is_null());
    if has_overlays || intro_title_overlay.is_some() {
        let overlays = args.get("overlays").filter(|v| !v.is_null());
        if let Some(ov) = overlays {
            require_object_keys(ov, &["track_name", "items"]).map_err(command_to_core)?;
        }
        let track_id = generated_id(project, key, "narration-overlay-track", 0);
        let track_name = overlays
            .and_then(|ov| ov.get("track_name"))
            .and_then(Value::as_str)
            .unwrap_or("串場字卡");
        operations.push(Operation::TrackAdd {
            track: make_track(track_id.clone(), track_name, TrackKind::Video),
        });
        created_track_ids.push(track_id.clone());

        let mut placed: Vec<(u64, u64)> = Vec::new();
        let mut overlay_clip_index = 0_usize;

        if let Some((title_asset_id, title_duration, title_fade_in, title_fade_out, scale, x, y)) =
            intro_title_overlay
        {
            let asset = find_asset(project, &title_asset_id)?;
            let clip_id = generated_id(project, key, "narration-overlay-clip", overlay_clip_index);
            overlay_clip_index += 1;
            let motion = build_overlay_motion(
                title_duration,
                frame,
                title_fade_in,
                title_fade_out,
                scale,
                x,
                y,
            );
            let clip = match asset.kind {
                AssetKind::Image => Clip::Image(ImageClip {
                    id: clip_id,
                    track_id: track_id.clone(),
                    asset_id: title_asset_id,
                    start_tick: 0,
                    duration_ticks: title_duration,
                    source_in_tick: 0,
                    motion,
                }),
                _ => Clip::Video(VideoClip {
                    id: clip_id,
                    track_id: track_id.clone(),
                    asset_id: title_asset_id,
                    start_tick: 0,
                    duration_ticks: title_duration,
                    source_in_tick: 0,
                    stream_index: stream_index(asset, StreamKind::Video)?,
                    motion,
                    audio_policy: VideoAudioPolicy::Muted,
                    hold_head_ticks: 0,
                    hold_tail_ticks: 0,
                }),
            };
            placed.push((0, title_duration));
            operations.push(Operation::ClipAdd { clip });
        }

        if let Some(ov) = overlays {
            let items = ov
                .get("items")
                .and_then(Value::as_array)
                .filter(|items| !items.is_empty() && items.len() <= 100)
                .ok_or_else(|| fail("overlays.items must contain 1 to 100 items"))?;
            for (index, item) in items.iter().enumerate() {
                let context = format!("overlays.items[{index}]");
                require_object_keys(
                    item,
                    &[
                        "asset_id",
                        "segment",
                        "offset_seconds",
                        "at_seconds",
                        "duration_seconds",
                        "fade_in_seconds",
                        "fade_out_seconds",
                        "scale",
                        "x",
                        "y",
                    ],
                )
                .map_err(command_to_core)?;
                let (visual, image_duration) = visual_from(project, item, frame, "overlay", &context)?;
                let at = match (item.get("segment"), item.get("at_seconds")) {
                    (Some(segment), None) => {
                        let segment = segment
                            .as_u64()
                            .map(|value| value as usize)
                            .filter(|value| *value < starts.len())
                            .ok_or_else(|| fail(format!("{context}.segment is out of range")))?;
                        let offset =
                            optional_seconds(item.get("offset_seconds"), "offset_seconds", true)?
                                .unwrap_or(0);
                        starts[segment] + offset
                    }
                    (None, Some(at)) => seconds_value_to_ticks(at, "at_seconds", true)?,
                    _ => {
                        return Err(fail(format!(
                            "{context} needs exactly one of segment or at_seconds"
                        )));
                    }
                };
                let at = round_to_grid(at, frame)?;
                let duration = match visual.kind {
                    AssetKind::Image => image_duration.ok_or_else(|| {
                        fail(format!("{context}: image overlays need duration_seconds"))
                    })?,
                    _ => visual.core,
                };
                let end = at + duration;
                if end > total {
                    return Err(fail(format!("{context} runs past the end of the story")));
                }
                if placed.iter().any(|(a, b)| at < *b && *a < end) {
                    return Err(fail(format!("{context} overlaps another overlay")));
                }
                placed.push((at, end));

                let fade_in = optional_seconds(item.get("fade_in_seconds"), "fade_in_seconds", true)?
                    .map(|ticks| round_to_grid(ticks, frame))
                    .transpose()?
                    .unwrap_or(0);
                let fade_out = optional_seconds(item.get("fade_out_seconds"), "fade_out_seconds", true)?
                    .map(|ticks| round_to_grid(ticks, frame))
                    .transpose()?
                    .unwrap_or(0);
                if fade_in + fade_out > duration {
                    return Err(fail(format!("{context} fades exceed the overlay duration")));
                }
                let scale = finite_number(item.get("scale"), 1.0, &format!("{context}.scale"))?;
                let x = finite_number(item.get("x"), 0.0, &format!("{context}.x"))?;
                let y = finite_number(item.get("y"), 0.0, &format!("{context}.y"))?;
                if !(-4.0..=4.0).contains(&x) {
                    return Err(fail(format!("{context}.x must be between -4.0 and 4.0")));
                }
                if !(-4.0..=4.0).contains(&y) {
                    return Err(fail(format!("{context}.y must be between -4.0 and 4.0")));
                }
                if !(1e-6..=16.0).contains(&scale) {
                    return Err(fail(format!("{context}.scale must be between 1e-6 and 16.0")));
                }

                let clip_id = generated_id(project, key, "narration-overlay-clip", overlay_clip_index);
                overlay_clip_index += 1;
                let motion = build_overlay_motion(duration, frame, fade_in, fade_out, scale, x, y);
                let asset = find_asset(project, &visual.asset_id)?;
                let clip = match visual.kind {
                    AssetKind::Image => Clip::Image(ImageClip {
                        id: clip_id,
                        track_id: track_id.clone(),
                        asset_id: visual.asset_id.clone(),
                        start_tick: at,
                        duration_ticks: duration,
                        source_in_tick: 0,
                        motion,
                    }),
                    _ => Clip::Video(VideoClip {
                        id: clip_id,
                        track_id: track_id.clone(),
                        asset_id: visual.asset_id.clone(),
                        start_tick: at,
                        duration_ticks: duration,
                        source_in_tick: visual.source_in,
                        stream_index: stream_index(asset, StreamKind::Video)?,
                        motion,
                        audio_policy: VideoAudioPolicy::Muted,
                        hold_head_ticks: 0,
                        hold_tail_ticks: 0,
                    }),
                };
                operations.push(Operation::ClipAdd { clip });
            }
        }
        overlay_track = Some(track_id);
    }

    if operations.len() > 500 {
        return Err(fail(format!(
            "narration assembly expands to {} operations; the transaction limit is 500",
            operations.len()
        )));
    }
    let data = json!({
        "created_track_ids":created_track_ids,
        "visual_track_id":visual_track,
        "narration_track_id":narration_track,
        "music_track_id":music_track,
        "overlay_track_id":overlay_track,
        "frame_ticks":frame,
        "dissolve_window_ticks":window,
        "lead_in_ticks":lead_in,
        "narration_end_tick":narration_end,
        "total_duration_ticks":total,
        "segment_offsets":segment_offsets,
        "cuts":cut_report,
        "native_cores":native_cores,
        "warnings":warnings,
    });
    Ok((operations, data))
}

#[cfg(test)]
mod tests {
    use crate::dispatch;
    use serde_json::{Value, json};
    use std::path::Path;
    use storycut_core::{
        Asset, AssetKind, Canvas, Clip, ProbeStatus, ProjectStore, Rational, Stream, StreamKind,
        TIMEBASE,
    };

    fn stream(kind: StreamKind) -> Stream {
        Stream {
            index: 0,
            kind,
            time_base: Rational {
                num: 1,
                den: 48_000,
            },
            sample_rate: (kind == StreamKind::Audio).then_some(48_000),
        }
    }

    fn asset(id: &str, kind: AssetKind, duration: Option<u64>) -> Asset {
        Asset {
            id: id.into(),
            kind,
            path: "fixture.bin".into(),
            probe_status: ProbeStatus::Probed,
            duration_ticks: duration,
            sha256: None,
            streams: match kind {
                AssetKind::Image => Vec::new(),
                AssetKind::Video => vec![stream(StreamKind::Video)],
                AssetKind::Audio => vec![stream(StreamKind::Audio)],
            },
        }
    }

    const SAMPLE: u64 = TIMEBASE / 48_000;
    const FRAME: u64 = TIMEBASE / 30;
    /// Narration segment lengths in samples (about 3.13, 10.74, 5.22, 8.28, 16.14 s).
    const SEGMENTS: [u64; 5] = [150_462, 515_340, 250_770, 397_476, 774_891];

    fn workspace(dir: &Path) -> String {
        let store = ProjectStore::create(
            dir,
            "story.storycut.json",
            "toutao",
            Canvas::default(),
            48_000,
            "create",
        )
        .unwrap();
        let mut assets = vec![
            asset("intro", AssetKind::Video, Some(241 * TIMEBASE / 24)),
            asset("native", AssetKind::Video, Some(241 * TIMEBASE / 24)),
            asset("img-a", AssetKind::Image, None),
            asset("img-b", AssetKind::Image, None),
            asset("outro", AssetKind::Image, None),
            asset("card", AssetKind::Image, None),
            asset("music", AssetKind::Audio, Some(200 * TIMEBASE)),
        ];
        for (index, samples) in SEGMENTS.iter().enumerate() {
            assets.push(asset(
                &format!("seg{index}"),
                AssetKind::Audio,
                Some(samples * SAMPLE),
            ));
        }
        store.import_assets(0, "import", false, assets).unwrap();
        store.snapshot().unwrap().project_id
    }

    fn request(project_id: &str, revision: u64, dry_run: bool) -> Value {
        json!({
            "project_id":project_id,"expected_revision":revision,"idempotency_key":"toutao-r1","dry_run":dry_run,
            "intro":{"asset_id":"intro","duration_seconds":10},
            "narration":{"segments":(0..5).map(|i| json!({"asset_id":format!("seg{i}")})).collect::<Vec<_>>()},
            "shots":[
                {"asset_id":"img-a","segments":[0,1]},
                {"asset_id":"native","segments":[2,2],"duration_seconds":10},
                {"asset_id":"img-b","segments":[3,4]}
            ],
            "outro":{"asset_id":"outro","duration_seconds":5},
            "transition":{"mode":"centered_dissolve","duration_seconds":0.4},
            "music":{"cues":[{"asset_id":"music","start_seconds":0,"fade_in_seconds":1.5,"fade_out_seconds":3}]},
            "overlays":{"items":[{"asset_id":"card","segment":1,"offset_seconds":0.3,"duration_seconds":4}]}
        })
    }

    fn load(dir: &Path, project_id: &str) -> storycut_core::Project {
        ProjectStore::load(dir, project_id)
            .unwrap()
            .snapshot()
            .unwrap()
    }

    #[test]
    fn toutao_shaped_story_keeps_length_and_native_cores() {
        let dir = tempfile::tempdir().unwrap();
        let project_id = workspace(dir.path());
        let preview = dispatch(
            dir.path(),
            "storycut_narration_assemble",
            request(&project_id, 1, true),
        )
        .unwrap();
        let data = &preview["data"];
        assert_eq!(data["committed"], false);
        // Intro: 300 native frames plus half a window before narration starts.
        assert_eq!(data["lead_in_ticks"], json!(306 * FRAME));
        assert_eq!(data["segment_offsets"][0]["start_tick"], json!(306 * FRAME));
        let body_end =
            (306 * FRAME + SEGMENTS.iter().sum::<u64>() * SAMPLE).div_ceil(FRAME) * FRAME;
        assert_eq!(data["total_duration_ticks"], json!(body_end + 150 * FRAME));

        let committed = dispatch(
            dir.path(),
            "storycut_narration_assemble",
            request(&project_id, 1, false),
        )
        .unwrap();
        assert_eq!(committed["data"]["committed"], true);
        let project = load(dir.path(), &project_id);
        assert_eq!(project.revision, 2);
        assert_eq!(project.duration_ticks(), body_end + 150 * FRAME);

        // Native cores never overlap a dissolve and stay exactly 300 frames.
        let cores = committed["data"]["native_cores"].as_array().unwrap();
        assert_eq!(cores.len(), 2);
        for core in cores {
            let a = core["core_start_tick"].as_u64().unwrap();
            let b = core["core_end_tick"].as_u64().unwrap();
            assert_eq!(b - a, 300 * FRAME);
            for transition in &project.transitions {
                let t0 = transition.start_tick;
                let t1 = t0 + transition.duration_ticks;
                assert!(
                    t1 <= a || t0 >= b,
                    "dissolve {t0}..{t1} touches core {a}..{b}"
                );
                assert_eq!(transition.duration_ticks, 12 * FRAME);
            }
        }
        // Cuts are contiguous and every dissolve is centred on a cut.
        let cuts = committed["data"]["cuts"].as_array().unwrap();
        assert_eq!(cuts.len(), 5);
        for pair in cuts.windows(2) {
            assert_eq!(pair[0]["cut_out_tick"], pair[1]["cut_in_tick"]);
        }
        assert_eq!(project.transitions.len(), 4);
        for (transition, cut) in project.transitions.iter().zip(&cuts[1..]) {
            assert_eq!(
                transition.start_tick + 6 * FRAME,
                cut["cut_in_tick"].as_u64().unwrap()
            );
        }
        let native = project
            .clips
            .iter()
            .find_map(|clip| match clip {
                Clip::Video(video) if video.asset_id == "native" => Some(video),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            (native.hold_head_ticks, native.hold_tail_ticks),
            (12 * FRAME, 12 * FRAME)
        );
        let music = project.tracks.iter().find(|t| t.name == "配樂").unwrap();
        let narration = project.tracks.iter().find(|t| t.name == "旁白").unwrap();
        assert_eq!(
            music.ducking.as_ref().unwrap().source_track_id,
            narration.id
        );

        // Same key and payload replays without adding clips.
        let replay = dispatch(
            dir.path(),
            "storycut_narration_assemble",
            request(&project_id, 1, false),
        )
        .unwrap();
        assert_eq!(replay["revision"], json!(2));
        assert_eq!(
            load(dir.path(), &project_id).clips.len(),
            project.clips.len()
        );
    }

    #[test]
    fn invalid_shot_coverage_and_edge_natives_are_rejected_without_writes() {
        let dir = tempfile::tempdir().unwrap();
        let project_id = workspace(dir.path());
        let mut gap = request(&project_id, 1, false);
        gap["shots"] = json!([
            {"asset_id":"img-a","segments":[0,1]},
            {"asset_id":"img-b","segments":[3,4]}
        ]);
        let error = dispatch(dir.path(), "storycut_narration_assemble", gap).unwrap_err();
        assert!(
            error.message.contains("continue at segment 2"),
            "{}",
            error.message
        );

        let mut edge = request(&project_id, 1, false);
        edge["idempotency_key"] = json!("edge");
        edge["shots"] = json!([
            {"asset_id":"native","segments":[0,1]},
            {"asset_id":"img-b","segments":[2,4]}
        ]);
        let error = dispatch(dir.path(), "storycut_narration_assemble", edge).unwrap_err();
        assert!(error.message.contains("align: start"), "{}", error.message);
        assert_eq!(load(dir.path(), &project_id).revision, 1);

        let mut fixed = request(&project_id, 1, false);
        fixed["idempotency_key"] = json!("aligned");
        fixed["shots"] = json!([
            {"asset_id":"native","segments":[0,1],"align":"start"},
            {"asset_id":"img-b","segments":[2,4]}
        ]);
        dispatch(dir.path(), "storycut_narration_assemble", fixed).unwrap();
        assert_eq!(load(dir.path(), &project_id).revision, 2);
    }

    #[test]
    fn shot_motion_matches_focal_motion_apply() {
        let dir = tempfile::tempdir().unwrap();
        let img_path = dir.path().join("img-a.png");
        let status = std::process::Command::new("ffmpeg")
            .args(["-y", "-f", "lavfi", "-i", "color=c=red:s=320x180:d=1", "-frames:v", "1", "-update", "1"])
            .arg(&img_path)
            .status()
            .expect("ffmpeg must be available for motion test");
        assert!(status.success());

        let store = ProjectStore::create(
            dir.path(),
            "story.storycut.json",
            "motion-test",
            Canvas::default(),
            48_000,
            "create",
        )
        .unwrap();

        let probe_a = storycut_render::probe_media(&img_path).unwrap();
        let mut assets = vec![
            Asset {
                id: "img-a".into(),
                kind: AssetKind::Image,
                path: "img-a.png".into(),
                probe_status: ProbeStatus::Probed,
                duration_ticks: None,
                sha256: Some(probe_a.sha256.clone()),
                streams: Vec::new(),
            },
            asset("outro", AssetKind::Image, None),
        ];
        for (index, samples) in SEGMENTS.iter().enumerate() {
            assets.push(asset(
                &format!("seg{index}"),
                AssetKind::Audio,
                Some(samples * SAMPLE),
            ));
        }
        store.import_assets(0, "import", false, assets).unwrap();
        let project_id = store.snapshot().unwrap().project_id;

        let motion_def = json!({
            "preset": "zoom_in",
            "focus": {"x": 0.6, "y": 0.4},
            "from_scale": 1.0,
            "to_scale": 1.2
        });

        // 1. Direct assembly with motion
        let req_with_motion = json!({
            "project_id": project_id,
            "expected_revision": 1,
            "idempotency_key": "narration-with-motion",
            "dry_run": false,
            "narration": {
                "segments": (0..5).map(|i| json!({"asset_id": format!("seg{i}")})).collect::<Vec<_>>()
            },
            "shots": [
                {
                    "asset_id": "img-a",
                    "segments": [0, 4],
                    "motion": motion_def
                }
            ],
            "transition": {"mode": "cut"}
        });
        let res1 = dispatch(dir.path(), "storycut_narration_assemble", req_with_motion).unwrap();
        assert_eq!(res1["ok"], true);
        let project1 = load(dir.path(), &project_id);
        let clip1 = project1.clips.iter().find(|c| c.asset_id() == "img-a").unwrap();
        let motion1 = match clip1 {
            Clip::Image(img) => &img.motion,
            _ => panic!("expected image clip"),
        };

        // 2. Assembly without motion on another project, then apply focal_motion_apply
        let dir2 = tempfile::tempdir().unwrap();
        std::fs::copy(&img_path, dir2.path().join("img-a.png")).unwrap();
        let store2 = ProjectStore::create(
            dir2.path(),
            "story.storycut.json",
            "motion-test-2",
            Canvas::default(),
            48_000,
            "create",
        )
        .unwrap();
        let mut assets2 = vec![
            Asset {
                id: "img-a".into(),
                kind: AssetKind::Image,
                path: "img-a.png".into(),
                probe_status: ProbeStatus::Probed,
                duration_ticks: None,
                sha256: Some(probe_a.sha256.clone()),
                streams: Vec::new(),
            },
            asset("outro", AssetKind::Image, None),
        ];
        for (index, samples) in SEGMENTS.iter().enumerate() {
            assets2.push(asset(
                &format!("seg{index}"),
                AssetKind::Audio,
                Some(samples * SAMPLE),
            ));
        }
        store2.import_assets(0, "import", false, assets2).unwrap();
        let project_id2 = store2.snapshot().unwrap().project_id;

        let req_without_motion = json!({
            "project_id": project_id2,
            "expected_revision": 1,
            "idempotency_key": "narration-without-motion",
            "dry_run": false,
            "narration": {
                "segments": (0..5).map(|i| json!({"asset_id": format!("seg{i}")})).collect::<Vec<_>>()
            },
            "shots": [
                {
                    "asset_id": "img-a",
                    "segments": [0, 4]
                }
            ],
            "transition": {"mode": "cut"}
        });
        let res2 = dispatch(dir2.path(), "storycut_narration_assemble", req_without_motion).unwrap();
        assert_eq!(res2["ok"], true);
        let project2 = load(dir2.path(), &project_id2);
        let clip2_id = project2.clips.iter().find(|c| c.asset_id() == "img-a").unwrap().id().to_owned();

        let focal_req = json!({
            "project_id": project_id2,
            "expected_revision": 2,
            "idempotency_key": "focal-apply",
            "dry_run": false,
            "targets": [
                {
                    "clip_id": clip2_id,
                    "focus": {"x": 0.6, "y": 0.4}
                }
            ],
            "preset": "zoom_in",
            "from_scale": 1.0,
            "to_scale": 1.2
        });
        let res_focal = dispatch(dir2.path(), "storycut_focal_motion_apply", focal_req).unwrap();
        assert_eq!(res_focal["ok"], true);
        let project2_after = load(dir2.path(), &project_id2);
        let clip2_after = project2_after.clips.iter().find(|c| c.asset_id() == "img-a").unwrap();
        let motion2 = match clip2_after {
            Clip::Image(img) => &img.motion,
            _ => panic!("expected image clip"),
        };

        // Verify that the motion keyframes from assemble and focal_motion_apply are completely identical!
        assert_eq!(motion1.keyframes, motion2.keyframes);
        assert_eq!(motion1.domain_duration_ticks, motion2.domain_duration_ticks);
        assert_eq!(motion1.sample_offset_tick, motion2.sample_offset_tick);
        assert_eq!(motion1.fit, motion2.fit);
        assert_eq!(motion1.avoid_exposed_edges, motion2.avoid_exposed_edges);
        assert_eq!(motion1.interpolation, motion2.interpolation);
    }

    #[test]
    fn narration_assemble_overlay_fade_and_intro_title() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProjectStore::create(
            dir.path(),
            "story.storycut.json",
            "overlay-test",
            Canvas::default(),
            48_000,
            "create",
        )
        .unwrap();

        let s = TIMEBASE;
        let mut assets = vec![
            asset("intro-video", AssetKind::Video, Some(10 * s)),
            asset("title-card", AssetKind::Image, None),
            asset("quote-card", AssetKind::Image, None),
            asset("shot-img", AssetKind::Image, None),
            asset("outro", AssetKind::Image, None),
        ];
        for (index, samples) in SEGMENTS.iter().enumerate() {
            assets.push(asset(
                &format!("seg{index}"),
                AssetKind::Audio,
                Some(samples * SAMPLE),
            ));
        }
        store.import_assets(0, "import", false, assets).unwrap();
        let project_id = store.snapshot().unwrap().project_id;

        let req = json!({
            "project_id": project_id,
            "expected_revision": 1,
            "idempotency_key": "assemble-with-overlays",
            "dry_run": false,
            "intro": {
                "asset_id": "intro-video",
                "title_overlay_asset_id": "title-card",
                "title_duration_seconds": 7.8,
                "title_fade_in_seconds": 1.0,
                "title_fade_out_seconds": 1.0,
                "title_scale": 0.85,
                "title_x": 0.05,
                "title_y": -0.1
            },
            "narration": {
                "segments": (0..5).map(|i| json!({"asset_id": format!("seg{i}")})).collect::<Vec<_>>()
            },
            "shots": [
                {
                    "asset_id": "shot-img",
                    "segments": [0, 4]
                }
            ],
            "overlays": {
                "track_name": "串場與引句",
                "items": [
                    {
                        "asset_id": "quote-card",
                        "at_seconds": 8.0,
                        "duration_seconds": 3.0,
                        "fade_in_seconds": 0.5,
                        "fade_out_seconds": 0.5,
                        "scale": 0.9,
                        "x": 0.0,
                        "y": 0.2
                    }
                ]
            },
            "transition": {"mode": "cut"}
        });

        let res = dispatch(dir.path(), "storycut_narration_assemble", req).unwrap();
        assert_eq!(res["ok"], true);
        let project = load(dir.path(), &project_id);

        let overlay_track_id = res["data"]["overlay_track_id"].as_str().unwrap();
        let overlay_clips: Vec<_> = project.clips.iter().filter(|c| c.track_id() == overlay_track_id).collect();
        assert_eq!(overlay_clips.len(), 2);

        // 1. Intro title overlay clip
        let title_clip = overlay_clips.iter().find(|c| c.asset_id() == "title-card").unwrap();
        assert_eq!(title_clip.start_tick(), 0);
        let frame = project.frame_ticks().unwrap();
        let expected_title_duration = (7.8 * s as f64).round() as u64;
        assert_eq!(title_clip.duration_ticks(), (expected_title_duration / frame) * frame);

        let title_motion = match title_clip {
            Clip::Image(img) => &img.motion,
            _ => panic!("expected image clip"),
        };
        assert_eq!(title_motion.avoid_exposed_edges, false);
        assert_eq!(title_motion.keyframes.len(), 4);
        assert_eq!(title_motion.keyframes[0].tick, 0);
        assert_eq!(title_motion.keyframes[0].opacity, 0.0);
        assert_eq!(title_motion.keyframes[0].scale, 0.85);
        assert_eq!(title_motion.keyframes[0].x, 0.05);
        assert_eq!(title_motion.keyframes[0].y, -0.1);

        let one_sec_tick = ((1.0 * s as f64).round() as u64 / frame) * frame;
        assert_eq!(title_motion.keyframes[1].tick, one_sec_tick);
        assert_eq!(title_motion.keyframes[1].opacity, 1.0);

        let fade_out_start_tick = title_clip.duration_ticks() - one_sec_tick;
        assert_eq!(title_motion.keyframes[2].tick, fade_out_start_tick);
        assert_eq!(title_motion.keyframes[2].opacity, 1.0);

        let last_domain_tick = title_clip.duration_ticks() - frame;
        assert_eq!(title_motion.keyframes[3].tick, last_domain_tick);
        assert_eq!(title_motion.keyframes[3].opacity, 0.0);

        // 2. Quote card overlay clip
        let quote_clip = overlay_clips.iter().find(|c| c.asset_id() == "quote-card").unwrap();
        let quote_motion = match quote_clip {
            Clip::Image(img) => &img.motion,
            _ => panic!("expected image clip"),
        };
        assert_eq!(quote_motion.avoid_exposed_edges, false);
        assert_eq!(quote_motion.keyframes.len(), 4);
        assert_eq!(quote_motion.keyframes[0].opacity, 0.0);
        assert_eq!(quote_motion.keyframes[1].opacity, 1.0);
        assert_eq!(quote_motion.keyframes[2].opacity, 1.0);
        assert_eq!(quote_motion.keyframes[3].opacity, 0.0);
        assert_eq!(quote_motion.keyframes[0].scale, 0.9);
        assert_eq!(quote_motion.keyframes[0].y, 0.2);
    }
}


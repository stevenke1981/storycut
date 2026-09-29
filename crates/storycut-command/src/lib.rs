//! The only application command entry point used by CLI, MCP, and desktop.

use fs2::FileExt;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fmt;
use std::fs;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use storycut_core::{
    Anchor, Asset, AssetKind, Canvas, Clip, FitMode, Interpolation, Keyframe, Link, MAX_TICKS,
    Motion, Operation, Project, ProjectStore, StreamKind, SubtitleFormat, TIMEBASE, Track,
    TrackKind, Transition, TransitionKind, VideoAudioPolicy,
};
use uuid::Uuid;

mod narration;

#[derive(Debug)]
pub struct CommandError {
    pub code: String,
    pub message: String,
    pub details: Value,
}

impl CommandError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            details: json!({}),
        }
    }
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for CommandError {}

pub fn supported_tools() -> Vec<String> {
    [
        "storycut_capabilities",
        "storycut_project_create",
        "storycut_project_open",
        "storycut_project_get",
        "storycut_project_save",
        "storycut_project_validate",
        "storycut_media_import",
        "storycut_media_list",
        "storycut_timeline_get",
        "storycut_timeline_apply",
        "storycut_storyboard_assemble",
        "storycut_focal_motion_apply",
        "storycut_narration_assemble",
        "storycut_track_add",
        "storycut_track_update",
        "storycut_clip_add",
        "storycut_clip_move",
        "storycut_clip_trim",
        "storycut_clip_split",
        "storycut_clip_remove",
        "storycut_motion_set",
        "storycut_audio_set",
        "storycut_transition_set",
        "storycut_link_create",
        "storycut_subtitle_import",
        "storycut_subtitle_export",
        "storycut_history_undo",
        "storycut_history_redo",
        "storycut_preview_frame",
        "storycut_preview_range",
        "storycut_render_start",
        "storycut_job_get",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

pub fn dispatch(workspace: &Path, tool: &str, args: Value) -> Result<Value, CommandError> {
    if let Some((op_name, allowed)) = shortcut_spec(tool) {
        let mut fields = vec![
            "project_id",
            "expected_revision",
            "idempotency_key",
            "dry_run",
        ];
        fields.extend_from_slice(allowed);
        require_object_keys(&args, &fields)?;
        let id = field_str(&args, "project_id")?;
        let expected = field_u64(&args, "expected_revision")?;
        let key = field_str(&args, "idempotency_key")?;
        let dry_run = field_bool(&args, "dry_run")?;
        let mut operation = args
            .as_object()
            .cloned()
            .ok_or_else(|| CommandError::new("INVALID_ARGUMENT", "Arguments must be an object"))?;
        for field in [
            "project_id",
            "expected_revision",
            "idempotency_key",
            "dry_run",
        ] {
            operation.remove(field);
        }
        operation.insert("op".to_owned(), Value::String(op_name.to_owned()));
        let apply = json!({
            "project_id":id,"expected_revision":expected,"idempotency_key":key,
            "dry_run":dry_run,"operations":[Value::Object(operation)]
        });
        return dispatch(workspace, "storycut_timeline_apply", apply);
    }
    match tool {
        "storycut_storyboard_assemble" => storyboard_assemble(workspace, &args),
        "storycut_focal_motion_apply" => focal_motion_apply(workspace, &args),
        "storycut_narration_assemble" => narration::narration_assemble(workspace, &args),
        "storycut_capabilities" => {
            require_object_keys(&args, &[])?;
            let ffmpeg_available = command_available("ffmpeg", "-version");
            let libass_available = if ffmpeg_available {
                Command::new("ffmpeg")
                    .arg("-filters")
                    .output()
                    .ok()
                    .is_some_and(|output| String::from_utf8_lossy(&output.stdout).contains(" ass "))
            } else {
                false
            };
            Ok(success(
                None,
                None,
                json!({
                    "implemented_tools": supported_tools(),
                "implemented_operations": [
                    "track.add","track.update","track.reorder","track.remove",
                    "clip.add","clip.move","clip.trim","clip.split","clip.remove",
                    "motion.set","audio.set","transition.set","transition.remove",
                    "link.create","link.remove","subtitle.cue.update",
                    "subtitle.cue.insert","subtitle.cue.remove","subtitle.shift"
                ],
                    "schema_version": "0.2.0-draft",
                    "daemon_version": "not-implemented",
                    "ffmpeg_available": ffmpeg_available,
                    "libass_available": libass_available,
                    "supported_protocol_versions": ["2025-11-25"],
                    "workspace_policy_summary": "Local workspace only"
                }),
            ))
        }
        "storycut_project_create" => {
            require_object_keys(
                &args,
                &[
                    "path",
                    "name",
                    "canvas",
                    "audio_sample_rate",
                    "idempotency_key",
                ],
            )?;
            let path = checked_path(workspace, field_str(&args, "path")?, false)?;
            let name = args
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| CommandError::new("INVALID_ARGUMENT", "Missing or invalid name"))?;
            let canvas: Canvas = serde_json::from_value(
                args.get("canvas")
                    .cloned()
                    .ok_or_else(|| CommandError::new("INVALID_ARGUMENT", "Missing canvas"))?,
            )
            .map_err(|e| CommandError::new("INVALID_ARGUMENT", e.to_string()))?;
            let rate = u32::try_from(field_u64(&args, "audio_sample_rate")?)
                .map_err(|_| CommandError::new("INVALID_ARGUMENT", "Invalid audio_sample_rate"))?;
            let key = field_str(&args, "idempotency_key")?;
            let store = ProjectStore::create(workspace, &path, name, canvas, rate, key)
                .map_err(core_error)?;
            let project = store.snapshot().map_err(core_error)?;
            let value = serde_json::to_value(&project).map_err(internal_error)?;
            Ok(success(
                Some(&project.project_id),
                Some(project.revision),
                json!({"project":value}),
            ))
        }
        "storycut_project_open" => {
            require_object_keys(&args, &["path"])?;
            let path = checked_path(workspace, field_str(&args, "path")?, true)?;
            let store = ProjectStore::open(&path).map_err(core_error)?;
            let project = store.snapshot().map_err(core_error)?;
            authorize_project_media(workspace, &path, &project)?;
            let value = serde_json::to_value(&project).map_err(internal_error)?;
            Ok(success(
                Some(&project.project_id),
                Some(project.revision),
                json!({"project":value}),
            ))
        }
        "storycut_project_get" => {
            require_object_keys(&args, &["project_id"])?;
            let id = field_str(&args, "project_id")?;
            let store = ProjectStore::load(workspace, id).map_err(core_error)?;
            let project = store.snapshot().map_err(core_error)?;
            let value = serde_json::to_value(&project).map_err(internal_error)?;
            Ok(success(
                Some(&project.project_id),
                Some(project.revision),
                json!({"project":value}),
            ))
        }
        "storycut_project_save" => {
            require_object_keys(
                &args,
                &["project_id", "expected_revision", "idempotency_key"],
            )?;
            let id = field_str(&args, "project_id")?;
            let expected = field_u64(&args, "expected_revision")?;
            let key = field_str(&args, "idempotency_key")?;
            let store = ProjectStore::load(workspace, id).map_err(core_error)?;
            let saved = store.save(expected, key).map_err(core_error)?;
            let path = store.project_path().map_err(core_error)?;
            Ok(with_projection_warning(
                success(
                    Some(id),
                    Some(saved.revision),
                    json!({
                        "saved_revision":saved.revision,"path":path.to_string_lossy()
                    }),
                ),
                saved.projection_warning.as_deref(),
            ))
        }
        "storycut_project_validate" => {
            require_object_keys(&args, &["project_id", "revision", "check_media"])?;
            let id = field_str(&args, "project_id")?;
            let revision = field_u64(&args, "revision")?;
            let check_media = field_bool(&args, "check_media")?;
            let store = ProjectStore::load(workspace, id).map_err(core_error)?;
            let project = store.snapshot().map_err(core_error)?;
            if project.revision != revision {
                return Err(CommandError::new(
                    "REVISION_CONFLICT",
                    format!("Current revision is {}", project.revision),
                ));
            }
            let mut issues = Vec::<Value>::new();
            if let Err(error) = storycut_core::validate_project(&project) {
                issues.push(json!({"code":"INVALID_PROJECT","severity":"error","message":error.to_string()}));
            }
            if check_media {
                let project_path = store.project_path().map_err(core_error)?;
                let base = project_path.parent().ok_or_else(|| {
                    CommandError::new("INTERNAL_ERROR", "Project has no directory")
                })?;
                for asset in &project.assets {
                    let path = Path::new(&asset.path);
                    let full = if path.is_absolute() {
                        path.to_path_buf()
                    } else {
                        base.join(path)
                    };
                    let checked = checked_path(workspace, &full.to_string_lossy(), true);
                    match checked.and_then(|p| storycut_render::probe_media(&p).map_err(render_error)) {
                        Ok(report) => {
                            if asset.sha256.as_deref() != Some(report.sha256.as_str()) {
                                issues.push(json!({"code":"MEDIA_CHANGED","severity":"error","message":format!("Asset {} has changed",asset.id)}));
                            }
                        }
                        Err(error) => issues.push(json!({"code":error.code,"severity":"error","message":format!("Asset {}: {}",asset.id,error.message)})),
                    }
                }
            }
            Ok(success(
                Some(id),
                Some(revision),
                json!({"valid":issues.is_empty(),"issues":issues}),
            ))
        }
        "storycut_media_list" => {
            require_object_keys(&args, &["project_id"])?;
            let id = field_str(&args, "project_id")?;
            let project = ProjectStore::load(workspace, id)
                .map_err(core_error)?
                .snapshot()
                .map_err(core_error)?;
            Ok(success(
                Some(&project.project_id),
                Some(project.revision),
                json!({"assets":project.assets}),
            ))
        }
        "storycut_media_import" => {
            require_object_keys(
                &args,
                &[
                    "project_id",
                    "expected_revision",
                    "idempotency_key",
                    "dry_run",
                    "paths",
                ],
            )?;
            let id = field_str(&args, "project_id")?;
            let expected = field_u64(&args, "expected_revision")?;
            let key = field_str(&args, "idempotency_key")?;
            let dry_run = field_bool(&args, "dry_run")?;
            let paths = field_array(&args, "paths")?;
            if paths.is_empty() || paths.len() > 200 {
                return Err(CommandError::new(
                    "INVALID_ARGUMENT",
                    "paths must contain 1 to 200 items",
                ));
            }
            let mut assets = Vec::<Asset>::with_capacity(paths.len());
            for (index, path) in paths.iter().enumerate() {
                let raw = path.as_str().ok_or_else(|| {
                    CommandError::new("INVALID_ARGUMENT", "Each path must be a string")
                })?;
                let checked = checked_path(workspace, raw, true)?;
                let probe = storycut_render::probe_media(&checked).map_err(render_error)?;
                let report = serde_json::to_value(probe).map_err(internal_error)?;
                let mut hasher = Sha256::new();
                hasher.update(key.as_bytes());
                hasher.update(index.to_le_bytes());
                hasher.update(checked.to_string_lossy().as_bytes());
                let asset_id = format!("asset-{}", &format!("{:x}", hasher.finalize())[..16]);
                let asset: Asset = serde_json::from_value(json!({
                    "id":asset_id,
                    "kind":report["kind"],
                    "path":checked.to_string_lossy(),
                    "probe_status":"probed",
                    "duration_ticks":report["duration_ticks"],
                    "sha256":report["sha256"],
                    "streams":report["streams"]
                }))
                .map_err(|e| {
                    CommandError::new("INTERNAL_ERROR", format!("Invalid probe result: {e}"))
                })?;
                assets.push(asset);
            }
            let store = ProjectStore::load(workspace, id).map_err(core_error)?;
            let applied = store
                .import_assets(expected, key, dry_run, assets.clone())
                .map_err(core_error)?;
            Ok(with_projection_warning(
                success(
                    Some(id),
                    Some(applied.revision),
                    json!({"committed":applied.applied,"assets":assets}),
                ),
                applied.projection_warning.as_deref(),
            ))
        }
        "storycut_timeline_get" => {
            require_object_keys(&args, &["project_id", "start_tick", "end_tick"])?;
            let id = field_str(&args, "project_id")?;
            let start = field_u64(&args, "start_tick")?;
            let end = match args.get("end_tick") {
                None | Some(Value::Null) => None,
                Some(value) => Some(value.as_u64().ok_or_else(|| {
                    CommandError::new(
                        "INVALID_ARGUMENT",
                        "end_tick must be a non-negative integer or null",
                    )
                })?),
            };
            if end.is_some_and(|v| v <= start) {
                return Err(CommandError::new(
                    "INVALID_ARGUMENT",
                    "end_tick must be later than start_tick",
                ));
            }
            let project = ProjectStore::load(workspace, id)
                .map_err(core_error)?
                .snapshot()
                .map_err(core_error)?;
            let duration = project_duration(&project)?;
            let range_end = end.unwrap_or(u64::MAX);
            let clips = project
                .clips
                .iter()
                .filter(|clip| {
                    clip.start_tick() < range_end && clip.end_tick().is_some_and(|at| at > start)
                })
                .collect::<Vec<_>>();
            let selected_ids = clips
                .iter()
                .map(|clip| clip.id())
                .collect::<std::collections::HashSet<_>>();
            let transitions = project
                .transitions
                .iter()
                .filter(|transition| {
                    transition.start_tick < range_end
                        && transition
                            .start_tick
                            .saturating_add(transition.duration_ticks)
                            > start
                })
                .collect::<Vec<_>>();
            let links = project
                .links
                .iter()
                .filter(|link| {
                    selected_ids.contains(link.clip_ids[0].as_str())
                        || selected_ids.contains(link.clip_ids[1].as_str())
                })
                .collect::<Vec<_>>();
            let subtitles = project
                .subtitles
                .iter()
                .filter(|subtitle| {
                    subtitle.cues.iter().any(|cue| {
                        subtitle.offset_tick.saturating_add(cue.start_tick) < range_end
                            && subtitle.offset_tick.saturating_add(cue.end_tick) > start
                    })
                })
                .collect::<Vec<_>>();
            let data = json!({
                "tracks": project.tracks,
                "clips": clips,
                "transitions": transitions,
                "links": links,
                "subtitles": subtitles,
                "duration_ticks": duration,
            });
            Ok(success(
                Some(&project.project_id),
                Some(project.revision),
                data,
            ))
        }
        "storycut_timeline_apply" => {
            require_object_keys(
                &args,
                &[
                    "project_id",
                    "expected_revision",
                    "idempotency_key",
                    "dry_run",
                    "operations",
                ],
            )?;
            let id = field_str(&args, "project_id")?;
            let expected = field_u64(&args, "expected_revision")?;
            let key = field_str(&args, "idempotency_key")?;
            let dry_run = field_bool(&args, "dry_run")?;
            let ops: Vec<Operation> =
                serde_json::from_value(Value::Array(field_array(&args, "operations")?.to_vec()))
                    .map_err(|e| CommandError::new("INVALID_ARGUMENT", e.to_string()))?;
            if ops.is_empty() || ops.len() > 500 {
                return Err(CommandError::new(
                    "INVALID_ARGUMENT",
                    "operations must contain 1 to 500 items",
                ));
            }
            let store = ProjectStore::load(workspace, id).map_err(core_error)?;
            let applied = store
                .apply(expected, key, dry_run, ops)
                .map_err(core_error)?;
            let diff = applied
                .changed_ids
                .iter()
                .map(|id| format!("changed:{id}"))
                .chain(applied.removed_ids.iter().map(|id| format!("removed:{id}")))
                .collect::<Vec<_>>();
            let data = json!({
                "committed":applied.applied,
                "base_revision":applied.base_revision,
                "changed_ids":applied.changed_ids,
                "removed_ids":applied.removed_ids,
                "duration_ticks":applied.duration_ticks,
                "diff":diff,
            });
            Ok(with_projection_warning(
                success(Some(id), Some(applied.revision), data),
                applied.projection_warning.as_deref(),
            ))
        }
        "storycut_history_undo" | "storycut_history_redo" => {
            require_object_keys(
                &args,
                &[
                    "project_id",
                    "expected_revision",
                    "idempotency_key",
                    "dry_run",
                ],
            )?;
            let id = field_str(&args, "project_id")?;
            let expected = field_u64(&args, "expected_revision")?;
            let key = field_str(&args, "idempotency_key")?;
            let dry_run = field_bool(&args, "dry_run")?;
            let store = ProjectStore::load(workspace, id).map_err(core_error)?;
            let applied = if tool == "storycut_history_undo" {
                store.undo(expected, key, dry_run)
            } else {
                store.redo(expected, key, dry_run)
            }
            .map_err(core_error)?;
            let diff = applied
                .changed_ids
                .iter()
                .map(|id| format!("changed:{id}"))
                .chain(applied.removed_ids.iter().map(|id| format!("removed:{id}")))
                .collect::<Vec<_>>();
            Ok(with_projection_warning(
                success(
                    Some(id),
                    Some(applied.revision),
                    json!({
                        "committed":applied.applied,"base_revision":applied.base_revision,
                        "changed_ids":applied.changed_ids,"removed_ids":applied.removed_ids,
                        "duration_ticks":applied.duration_ticks,"diff":diff
                    }),
                ),
                applied.projection_warning.as_deref(),
            ))
        }
        "storycut_subtitle_import" => {
            require_object_keys(
                &args,
                &[
                    "project_id",
                    "expected_revision",
                    "idempotency_key",
                    "dry_run",
                    "path",
                    "track_id",
                    "offset_tick",
                ],
            )?;
            let id = field_str(&args, "project_id")?;
            let expected = field_u64(&args, "expected_revision")?;
            let key = field_str(&args, "idempotency_key")?;
            let dry_run = field_bool(&args, "dry_run")?;
            let path = checked_path(workspace, field_str(&args, "path")?, true)?;
            let track_id = field_str(&args, "track_id")?;
            let offset = field_u64(&args, "offset_tick")?;
            let mut subtitle = storycut_subtitle::parse_subtitle(&path, track_id, offset)
                .map_err(subtitle_error)?;
            let mut hash = Sha256::new();
            hash.update(id.as_bytes());
            hash.update(b"\0subtitle\0");
            hash.update(key.as_bytes());
            hash.update(b"\0");
            hash.update(path.to_string_lossy().as_bytes());
            let fingerprint = format!("{:x}", hash.finalize());
            subtitle.id = format!("subtitle-{}", &fingerprint[..20]);
            for (index, cue) in subtitle.cues.iter_mut().enumerate() {
                cue.id = format!("cue-{}-{index}", &fingerprint[..16]);
            }
            let store = ProjectStore::load(workspace, id).map_err(core_error)?;
            let applied = store
                .import_subtitles(expected, key, dry_run, vec![subtitle.clone()])
                .map_err(core_error)?;
            Ok(with_projection_warning(
                success(
                    Some(id),
                    Some(applied.revision),
                    json!({
                        "committed":applied.applied,"document":subtitle
                    }),
                ),
                applied.projection_warning.as_deref(),
            ))
        }
        "storycut_subtitle_export" => {
            require_object_keys(
                &args,
                &[
                    "project_id",
                    "revision",
                    "path",
                    "overwrite",
                    "idempotency_key",
                    "document_id",
                    "format",
                    "allow_lossy",
                ],
            )?;
            let id = field_str(&args, "project_id")?;
            let revision = field_u64(&args, "revision")?;
            let key = field_str(&args, "idempotency_key")?;
            let document_id = field_str(&args, "document_id")?;
            if !valid_id(key) {
                return Err(CommandError::new(
                    "INVALID_ARGUMENT",
                    "Invalid idempotency_key",
                ));
            }
            let format: SubtitleFormat = serde_json::from_value(json!(field_str(&args, "format")?))
                .map_err(|e| CommandError::new("INVALID_ARGUMENT", e.to_string()))?;
            let allow_lossy = field_bool(&args, "allow_lossy")?;
            if field_bool(&args, "overwrite")? {
                return Err(CommandError::new(
                    "OVERWRITE_DENIED",
                    "Overwriting is not enabled",
                ));
            }
            let output = checked_path(workspace, field_str(&args, "path")?, false)?;
            let export_id = stable_export_id(id, key);
            let exports = private_dir(workspace, "exports", false)?;
            let record_path = exports.join(format!("{export_id}.json"));
            if record_path.exists() {
                let _lock = lock_job(&exports, &export_id, true)?;
                return replay_export(&record_path, &args, &output, id, revision)?.ok_or_else(
                    || CommandError::new("INTERNAL_ERROR", "Export record disappeared"),
                );
            }
            let store = ProjectStore::load(workspace, id).map_err(core_error)?;
            let project = store.snapshot().map_err(core_error)?;
            if project.revision != revision {
                return Err(CommandError::new(
                    "REVISION_CONFLICT",
                    format!("Current revision is {}", project.revision),
                ));
            }
            let subtitle = project
                .subtitles
                .iter()
                .find(|subtitle| subtitle.id == document_id)
                .ok_or_else(|| CommandError::new("NOT_FOUND", "Subtitle document not found"))?;
            let report = storycut_subtitle::assess_lossiness(subtitle, format);
            if report.is_lossy() && !allow_lossy {
                return Err(CommandError::new(
                    "UNSUPPORTED_FEATURE",
                    "Subtitle export would lose syntax or styling; set allow_lossy after reviewing the format",
                ));
            }
            let exported =
                storycut_subtitle::export_subtitle(subtitle, format).map_err(subtitle_error)?;
            let content = exported.content;
            let digest = hex_sha256(content.as_bytes());
            let root = fs::canonicalize(workspace)
                .map_err(|e| CommandError::new("PATH_DENIED", e.to_string()))?;
            let relative = output
                .strip_prefix(&root)
                .map_err(|_| CommandError::new("PATH_DENIED", "Export escaped workspace"))?;
            let data = json!({
                "artifact":{"artifact_id":export_id,"relative_path":relative.to_string_lossy(),
                    "mime_type":"text/plain; charset=utf-8","sha256":digest,
                    "size_bytes":content.len()},
                "loss_report":exported.lossiness.items.iter()
                    .map(|item| format!("{}: {}", item.code, item.message)).collect::<Vec<_>>()
            });
            private_dir(workspace, "exports", true)?;
            let _lock = lock_job(&exports, &export_id, true)?;
            if let Some(response) = replay_export(&record_path, &args, &output, id, revision)? {
                return Ok(response);
            }
            if output.exists() {
                return Err(CommandError::new(
                    "OVERWRITE_DENIED",
                    "Output already exists",
                ));
            }
            let record = json!({"request":args,"revision":revision,"content":content,"data":data});
            write_json_new(&record_path, &record)?;
            write_new_output(&output, content.as_bytes())?;
            Ok(success(Some(id), Some(revision), data))
        }
        "storycut_preview_frame" => {
            require_object_keys(
                &args,
                &[
                    "project_id",
                    "revision",
                    "idempotency_key",
                    "width",
                    "include_subtitles",
                    "at_tick",
                ],
            )?;
            let id = field_str(&args, "project_id")?;
            let revision = field_u64(&args, "revision")?;
            let key = field_str(&args, "idempotency_key")?;
            if !valid_id(key) {
                return Err(CommandError::new(
                    "INVALID_ARGUMENT",
                    "Invalid idempotency_key",
                ));
            }
            let width = u32::try_from(field_u64(&args, "width")?)
                .map_err(|_| CommandError::new("INVALID_ARGUMENT", "Invalid width"))?;
            if !(160..=1920).contains(&width) {
                return Err(CommandError::new(
                    "INVALID_ARGUMENT",
                    "width must be within 160..=1920",
                ));
            }
            let include_subtitles = field_bool(&args, "include_subtitles")?;
            let at_tick = field_u64(&args, "at_tick")?;
            let job_id = stable_preview_id(id, key);
            let jobs = job_dir(workspace)?;
            let initial = jobs.join(format!("{job_id}.initial.json"));
            let result = jobs.join(format!("{job_id}.result.json"));
            if initial.exists() {
                let _lock = lock_job(&jobs, &job_id, true)?;
                return existing_job_response(workspace, &jobs, &job_id, &args, id, revision)?
                    .ok_or_else(|| CommandError::new("INTERNAL_ERROR", "Job disappeared"));
            }
            let store = ProjectStore::load(workspace, id).map_err(core_error)?;
            let project = store.snapshot().map_err(core_error)?;
            if project.revision != revision {
                return Err(CommandError::new(
                    "REVISION_CONFLICT",
                    format!("Current revision is {}", project.revision),
                ));
            }
            if at_tick >= project.duration_ticks() {
                return Err(CommandError::new(
                    "INVALID_ARGUMENT",
                    "at_tick is outside the project duration",
                ));
            }
            authorize_project_media(
                workspace,
                &store.project_path().map_err(core_error)?,
                &project,
            )?;
            let mut height = ((u64::from(width) * u64::from(project.canvas.height)
                + u64::from(project.canvas.width) / 2)
                / u64::from(project.canvas.width)) as u32;
            height = height.clamp(16, 1920);
            if height % 2 != 0 {
                height = height.saturating_add(1).min(1920);
            }
            let root = fs::canonicalize(workspace)
                .map_err(|e| CommandError::new("PATH_DENIED", e.to_string()))?;
            let previews = private_dir(workspace, "previews", true)?;
            private_dir(workspace, "jobs", true)?;
            let _lock = lock_job(&jobs, &job_id, true)?;
            if let Some(response) =
                existing_job_response(workspace, &jobs, &job_id, &args, id, revision)?
            {
                return Ok(response);
            }
            let output = previews.join(format!("{job_id}.png"));
            let stage = jobs.join(format!("{job_id}.stage.png"));
            let running = json!({"job_id":job_id,"project_id":id,"source_revision":revision,
                "kind":"preview_frame","state":"running","progress":null,"last_event_seq":1,"artifacts":[],"failure_code":null});
            write_json_new(&initial, &json!({"request":args,"job":running}))?;
            let project_json = serde_json::to_value(&project).map_err(internal_error)?;
            let project_path = store.project_path().map_err(core_error)?;
            let rendered = storycut_render::render_frame_with_root(
                &project_json,
                &project_path,
                workspace,
                &stage,
                at_tick,
                width,
                height,
                include_subtitles,
            );
            let job = match rendered {
                Ok(report) => {
                    let size = fs::metadata(&stage)
                        .map_err(|e| CommandError::new("INTERNAL_ERROR", e.to_string()))?
                        .len();
                    let relative = output.strip_prefix(&root).map_err(|_| {
                        CommandError::new("PATH_DENIED", "Preview escaped workspace")
                    })?;
                    let ready_job = json!({"job_id":job_id,"project_id":id,"source_revision":revision,
                        "kind":"preview_frame","state":"succeeded","progress":1.0,"last_event_seq":2,"failure_code":null,
                        "artifacts":[{"artifact_id":format!("artifact-{job_id}"),"relative_path":relative.to_string_lossy(),
                            "mime_type":"image/png","sha256":report.output_sha256,"size_bytes":size}]});
                    write_ready_job(&jobs, &job_id, &ready_job)?;
                    let completed = finalize_ready_job(workspace, &jobs, &job_id)?;
                    return Ok(success(
                        Some(id),
                        Some(revision),
                        json!({"job":completed["job"]}),
                    ));
                }
                Err(error) => {
                    let mapped = render_error(error);
                    json!({"job_id":job_id,"project_id":id,"source_revision":revision,
                        "kind":"preview_frame","state":"failed","progress":null,"last_event_seq":2,"artifacts":[],"failure_code":mapped.code})
                }
            };
            write_json_new(&result, &json!({"job":job}))?;
            Ok(success(Some(id), Some(revision), json!({"job":job})))
        }
        "storycut_preview_range" => {
            require_object_keys(
                &args,
                &[
                    "project_id",
                    "revision",
                    "idempotency_key",
                    "width",
                    "include_subtitles",
                    "start_tick",
                    "duration_ticks",
                    "master_loudness",
                ],
            )?;
            let id = field_str(&args, "project_id")?;
            let revision = field_u64(&args, "revision")?;
            let key = field_str(&args, "idempotency_key")?;
            if !valid_id(key) {
                return Err(CommandError::new(
                    "INVALID_ARGUMENT",
                    "Invalid idempotency_key",
                ));
            }
            let width = u32::try_from(field_u64(&args, "width")?)
                .map_err(|_| CommandError::new("INVALID_ARGUMENT", "Invalid width"))?;
            if !(160..=1920).contains(&width) || width % 2 != 0 {
                return Err(CommandError::new(
                    "INVALID_ARGUMENT",
                    "width must be an even value within 160..=1920",
                ));
            }
            let include_subtitles = field_bool(&args, "include_subtitles")?;
            let start_tick = field_u64(&args, "start_tick")?;
            let duration_ticks = field_u64(&args, "duration_ticks")?;
            if duration_ticks == 0 {
                return Err(CommandError::new(
                    "INVALID_ARGUMENT",
                    "duration_ticks must be greater than zero",
                ));
            }
            let end_tick = start_tick.checked_add(duration_ticks).ok_or_else(|| {
                CommandError::new("INVALID_ARGUMENT", "Preview range end overflow")
            })?;
            let job_id = stable_preview_range_id(id, key);
            let jobs = job_dir(workspace)?;
            let initial = jobs.join(format!("{job_id}.initial.json"));
            if initial.exists() {
                let _lock = lock_job(&jobs, &job_id, true)?;
                return existing_job_response(workspace, &jobs, &job_id, &args, id, revision)?
                    .ok_or_else(|| CommandError::new("INTERNAL_ERROR", "Job disappeared"));
            }
            let store = ProjectStore::load(workspace, id).map_err(core_error)?;
            let project = store.snapshot().map_err(core_error)?;
            if project.revision != revision {
                return Err(CommandError::new(
                    "REVISION_CONFLICT",
                    format!("Current revision is {}", project.revision),
                ));
            }
            let project_duration = project.duration_ticks();
            let frame_ticks = project.frame_ticks().ok_or_else(|| {
                CommandError::new(
                    "INVALID_ARGUMENT",
                    "Project frame rate is not representable",
                )
            })?;
            if start_tick >= project_duration || end_tick > project_duration {
                return Err(CommandError::new(
                    "INVALID_ARGUMENT",
                    "Preview range is outside the project duration",
                ));
            }
            if start_tick % frame_ticks != 0 {
                return Err(CommandError::new(
                    "INVALID_ARGUMENT",
                    "start_tick must be aligned to the project frame grid",
                ));
            }
            authorize_project_media(
                workspace,
                &store.project_path().map_err(core_error)?,
                &project,
            )?;
            let mut height = ((u64::from(width) * u64::from(project.canvas.height)
                + u64::from(project.canvas.width) / 2)
                / u64::from(project.canvas.width)) as u32;
            height = height.clamp(16, 1920);
            if height % 2 != 0 {
                height = height.saturating_add(1).min(1920);
            }
            let root = fs::canonicalize(workspace)
                .map_err(|e| CommandError::new("PATH_DENIED", e.to_string()))?;
            let previews = private_dir(workspace, "previews", true)?;
            private_dir(workspace, "jobs", true)?;
            let _lock = lock_job(&jobs, &job_id, true)?;
            if let Some(response) =
                existing_job_response(workspace, &jobs, &job_id, &args, id, revision)?
            {
                return Ok(response);
            }
            let output = previews.join(format!("{job_id}.mp4"));
            let stage = jobs.join(format!("{job_id}.stage.mp4"));
            let running = json!({"job_id":job_id,"project_id":id,"source_revision":revision,
                "kind":"preview_range","state":"running","progress":null,"last_event_seq":1,"artifacts":[],"failure_code":null});
            write_json_new(&initial, &json!({"request":args,"job":running}))?;

            let mut resized_project = serde_json::to_value(&project).map_err(internal_error)?;
            let canvas = resized_project
                .get_mut("canvas")
                .and_then(Value::as_object_mut)
                .ok_or_else(|| CommandError::new("INTERNAL_ERROR", "Project canvas is missing"))?;
            canvas.insert("width".into(), Value::from(width));
            canvas.insert("height".into(), Value::from(height));
            let mut options = json!({
                "range_start_tick":start_tick,
                "range_end_tick":end_tick,
                "subtitle_mode":if include_subtitles {"burn"} else {"none"},
                "encoder":"h264_cpu",
                "overwrite":false
            });
            if let Some(loudness) = args.get("master_loudness").filter(|v| !v.is_null()) {
                options["master_loudness"] = loudness.clone();
            }
            let project_path = store.project_path().map_err(core_error)?;
            let rendered = storycut_render::render_with_root(
                &resized_project,
                &project_path,
                workspace,
                &stage,
                &options,
            );
            let job = match rendered {
                Ok(report) => {
                    let metadata = fs::metadata(&stage).map_err(|e| {
                        CommandError::new("INTERNAL_ERROR", format!("Preview range missing: {e}"))
                    })?;
                    let relative = output.strip_prefix(&root).map_err(|_| {
                        CommandError::new("PATH_DENIED", "Preview range escaped workspace")
                    })?;
                    let ready_job = json!({
                        "job_id":job_id,"project_id":id,"source_revision":revision,
                        "kind":"preview_range","state":"succeeded","progress":1.0,"last_event_seq":2,"failure_code":null,
                        "artifacts":[{"artifact_id":format!("artifact-{job_id}"),"relative_path":relative.to_string_lossy(),
                            "mime_type":"video/mp4","sha256":report.output_sha256,"size_bytes":metadata.len()}]
                    });
                    write_ready_job(&jobs, &job_id, &ready_job)?;
                    let completed = finalize_ready_job(workspace, &jobs, &job_id)?;
                    let mut response =
                        success(Some(id), Some(revision), json!({"job":completed["job"]}));
                    response["warnings"] = json!([{"code":"SYNCHRONOUS_PREVIEW","message":"Range preview currently completes before preview_range returns; cancellation is not available."}]);
                    return Ok(response);
                }
                Err(error) => {
                    let mapped = render_error(error);
                    json!({"job_id":job_id,"project_id":id,"source_revision":revision,
                        "kind":"preview_range","state":"failed","progress":null,"last_event_seq":2,"artifacts":[],"failure_code":mapped.code})
                }
            };
            write_json_new(
                &jobs.join(format!("{job_id}.result.json")),
                &json!({"job":job}),
            )?;
            let mut response = success(Some(id), Some(revision), json!({"job":job}));
            response["warnings"] = json!([{"code":"SYNCHRONOUS_PREVIEW","message":"Range preview currently completes before preview_range returns; cancellation is not available."}]);
            Ok(response)
        }
        "storycut_render_start" => {
            require_object_keys(
                &args,
                &[
                    "project_id",
                    "revision",
                    "path",
                    "overwrite",
                    "idempotency_key",
                    "subtitle_mode",
                    "encoder",
                    "range_start_tick",
                    "range_end_tick",
                    "master_loudness",
                ],
            )?;
            let id = field_str(&args, "project_id")?;
            let revision = field_u64(&args, "revision")?;
            let key = field_str(&args, "idempotency_key")?;
            if !valid_id(key) {
                return Err(CommandError::new(
                    "INVALID_ARGUMENT",
                    "Invalid idempotency_key",
                ));
            }
            let subtitle_mode = field_str(&args, "subtitle_mode")?;
            if !matches!(subtitle_mode, "none" | "burn") {
                return Err(CommandError::new(
                    "INVALID_ARGUMENT",
                    "Invalid subtitle_mode",
                ));
            }
            let encoder = field_str(&args, "encoder")?;
            if encoder != "h264_cpu" {
                return Err(CommandError::new(
                    "UNSUPPORTED_FEATURE",
                    "Only h264_cpu is available",
                ));
            }
            let range_start = field_u64(&args, "range_start_tick")?;
            let range_end = match args.get("range_end_tick") {
                None | Some(Value::Null) => None,
                Some(value) => Some(value.as_u64().ok_or_else(|| {
                    CommandError::new(
                        "INVALID_ARGUMENT",
                        "range_end_tick must be an integer or null",
                    )
                })?),
            };
            if range_end.is_some_and(|end| end <= range_start) {
                return Err(CommandError::new(
                    "INVALID_ARGUMENT",
                    "range_end_tick must be later than range_start_tick",
                ));
            }
            let output = checked_path(workspace, field_str(&args, "path")?, false)?;
            if field_bool(&args, "overwrite")? {
                return Err(CommandError::new(
                    "OVERWRITE_DENIED",
                    "Overwriting an existing output is not enabled",
                ));
            }
            let job_id = stable_job_id(id, key);
            let jobs = job_dir(workspace)?;
            let initial = jobs.join(format!("{job_id}.initial.json"));
            let result = jobs.join(format!("{job_id}.result.json"));
            if initial.exists() {
                let _lock = lock_job(&jobs, &job_id, true)?;
                return existing_job_response(workspace, &jobs, &job_id, &args, id, revision)?
                    .ok_or_else(|| CommandError::new("INTERNAL_ERROR", "Job disappeared"));
            }
            let store = ProjectStore::load(workspace, id).map_err(core_error)?;
            let project = store.snapshot().map_err(core_error)?;
            if project.revision != revision {
                return Err(CommandError::new(
                    "REVISION_CONFLICT",
                    format!("Current revision is {}", project.revision),
                ));
            }
            let project_duration = project.duration_ticks();
            if range_start >= project_duration
                || range_end.is_some_and(|end| end > project_duration)
            {
                return Err(CommandError::new(
                    "INVALID_ARGUMENT",
                    "Render range is outside the project duration",
                ));
            }
            authorize_project_media(
                workspace,
                &store.project_path().map_err(core_error)?,
                &project,
            )?;
            if output.exists() {
                return Err(CommandError::new(
                    "OVERWRITE_DENIED",
                    "Output already exists",
                ));
            }
            private_dir(workspace, "jobs", true)?;
            let _lock = lock_job(&jobs, &job_id, true)?;
            if let Some(response) =
                existing_job_response(workspace, &jobs, &job_id, &args, id, revision)?
            {
                return Ok(response);
            }
            if output.exists() {
                return Err(CommandError::new(
                    "OVERWRITE_DENIED",
                    "Output already exists",
                ));
            }
            let running = json!({
                "job_id":job_id,"project_id":id,"source_revision":revision,"kind":"render",
                "state":"running","progress":null,"last_event_seq":1,"artifacts":[],"failure_code":null
            });
            write_json_new(&initial, &json!({"request":args,"job":running}))?;
            let project_json = serde_json::to_value(&project).map_err(internal_error)?;
            let project_path = store.project_path().map_err(core_error)?;
            let stage = jobs.join(format!("{job_id}.stage.mp4"));
            let render_result = storycut_render::render_with_root(
                &project_json,
                &project_path,
                workspace,
                &stage,
                &args,
            );
            let job = match render_result {
                Ok(report) => {
                    let metadata = fs::metadata(&stage).map_err(|e| {
                        CommandError::new("INTERNAL_ERROR", format!("Rendered output missing: {e}"))
                    })?;
                    let root = fs::canonicalize(workspace)
                        .map_err(|e| CommandError::new("PATH_DENIED", e.to_string()))?;
                    let relative = output.strip_prefix(&root).map_err(|_| {
                        CommandError::new("PATH_DENIED", "Output escaped workspace")
                    })?;
                    let ready_job = json!({
                        "job_id":job_id,"project_id":id,"source_revision":revision,"kind":"render",
                        "state":"succeeded","progress":1.0,"last_event_seq":2,"failure_code":null,
                        "artifacts":[{"artifact_id":format!("artifact-{job_id}"),"relative_path":relative.to_string_lossy(),
                            "mime_type":"video/mp4","sha256":report.output_sha256,"size_bytes":metadata.len()}]
                    });
                    write_ready_job(&jobs, &job_id, &ready_job)?;
                    let completed = finalize_ready_job(workspace, &jobs, &job_id)?;
                    let mut response =
                        success(Some(id), Some(revision), json!({"job":completed["job"]}));
                    response["warnings"] = json!([{"code":"SYNCHRONOUS_RENDER","message":"Rendering currently completes before render_start returns; cancellation is not available."}]);
                    return Ok(response);
                }
                Err(error) => {
                    let mapped = render_error(error);
                    json!({
                        "job_id":job_id,"project_id":id,"source_revision":revision,"kind":"render",
                        "state":"failed","progress":null,"last_event_seq":2,"artifacts":[],"failure_code":mapped.code
                    })
                }
            };
            write_json_new(&result, &json!({"job":job}))?;
            let mut response = success(Some(id), Some(revision), json!({"job":job}));
            response["warnings"] = json!([{"code":"SYNCHRONOUS_RENDER","message":"Rendering currently completes before render_start returns; cancellation and restart recovery are not available."}]);
            Ok(response)
        }
        "storycut_job_get" => {
            require_object_keys(&args, &["job_id", "include_preview"])?;
            let id = field_str(&args, "job_id")?;
            let _ = field_bool(&args, "include_preview")?;
            if !valid_id(id) {
                return Err(CommandError::new("INVALID_ARGUMENT", "Invalid job_id"));
            }
            let jobs = job_dir(workspace)?;
            let result = jobs.join(format!("{id}.result.json"));
            let initial = jobs.join(format!("{id}.initial.json"));
            let record = if result.exists() {
                read_job_file(&result)?
            } else {
                if !initial.exists() {
                    return Err(CommandError::new("NOT_FOUND", "Job not found"));
                }
                match lock_job(&jobs, id, false)? {
                    Some(_lock) => completed_or_interrupted_job(workspace, &jobs, id)?,
                    None => read_job_file(&initial)?,
                }
            };
            let job = record["job"].clone();
            if job.is_null() {
                return Err(CommandError::new(
                    "INTERNAL_ERROR",
                    "Job record is malformed",
                ));
            }
            Ok(success(
                job["project_id"].as_str(),
                job["source_revision"].as_u64(),
                json!({"job":job}),
            ))
        }
        _ => Err(CommandError::new(
            "UNSUPPORTED_FEATURE",
            format!("Tool {tool} is not implemented"),
        )),
    }
}

fn semantic_identity(args: &Value) -> Value {
    let mut identity = args.as_object().cloned().unwrap_or_default();
    for field in [
        "project_id",
        "expected_revision",
        "idempotency_key",
        "dry_run",
    ] {
        identity.remove(field);
    }
    Value::Object(identity)
}

fn storyboard_assemble(workspace: &Path, args: &Value) -> Result<Value, CommandError> {
    require_object_keys(
        args,
        &[
            "project_id",
            "expected_revision",
            "idempotency_key",
            "dry_run",
            "items",
            "visual_track_id",
            "transition",
            "original_video_audio",
            "audio_tracks",
        ],
    )?;
    let project_id = field_str(args, "project_id")?;
    let expected_revision = field_u64(args, "expected_revision")?;
    let idempotency_key = field_str(args, "idempotency_key")?;
    let dry_run = field_bool(args, "dry_run")?;
    let store = ProjectStore::load(workspace, project_id).map_err(core_error)?;
    let identity = semantic_identity(args);
    let planned = store
        .apply_planned(
            expected_revision,
            idempotency_key,
            dry_run,
            "storycut_storyboard_assemble",
            identity,
            |project| plan_storyboard(workspace, project, args, idempotency_key),
        )
        .map_err(core_error)?;
    let mut data = planned.data.as_object().cloned().unwrap_or_default();
    data.insert("committed".into(), json!(planned.result.applied));
    data.insert("base_revision".into(), json!(planned.result.base_revision));
    data.insert("changed_ids".into(), json!(planned.result.changed_ids));
    data.insert("removed_ids".into(), json!(planned.result.removed_ids));
    data.insert(
        "duration_ticks".into(),
        json!(planned.result.duration_ticks),
    );
    data.insert(
        "diff".into(),
        json!(
            planned
                .result
                .changed_ids
                .iter()
                .map(|id| format!("changed:{id}"))
                .chain(
                    planned
                        .result
                        .removed_ids
                        .iter()
                        .map(|id| format!("removed:{id}"))
                )
                .collect::<Vec<_>>()
        ),
    );
    Ok(with_projection_warning(
        success(
            Some(project_id),
            Some(planned.result.revision),
            Value::Object(data),
        ),
        planned.result.projection_warning.as_deref(),
    ))
}

fn focal_motion_apply(workspace: &Path, args: &Value) -> Result<Value, CommandError> {
    require_object_keys(
        args,
        &[
            "project_id",
            "expected_revision",
            "idempotency_key",
            "dry_run",
            "targets",
            "preset",
            "from_scale",
            "to_scale",
            "pan_amount",
        ],
    )?;
    let project_id = field_str(args, "project_id")?;
    let expected_revision = field_u64(args, "expected_revision")?;
    let idempotency_key = field_str(args, "idempotency_key")?;
    let dry_run = field_bool(args, "dry_run")?;
    let store = ProjectStore::load(workspace, project_id).map_err(core_error)?;
    let identity = semantic_identity(args);
    let planned = store
        .apply_planned(
            expected_revision,
            idempotency_key,
            dry_run,
            "storycut_focal_motion_apply",
            identity,
            |project| plan_focal_motion(workspace, project, args),
        )
        .map_err(core_error)?;
    let mut data = planned.data.as_object().cloned().unwrap_or_default();
    data.insert("committed".into(), json!(planned.result.applied));
    data.insert("base_revision".into(), json!(planned.result.base_revision));
    data.insert("changed_ids".into(), json!(planned.result.changed_ids));
    data.insert("removed_ids".into(), json!(planned.result.removed_ids));
    data.insert(
        "duration_ticks".into(),
        json!(planned.result.duration_ticks),
    );
    data.insert(
        "diff".into(),
        json!(
            planned
                .result
                .changed_ids
                .iter()
                .map(|id| format!("changed:{id}"))
                .collect::<Vec<_>>()
        ),
    );
    Ok(with_projection_warning(
        success(
            Some(project_id),
            Some(planned.result.revision),
            Value::Object(data),
        ),
        planned.result.projection_warning.as_deref(),
    ))
}

fn plan_storyboard(
    _workspace: &Path,
    project: &Project,
    args: &Value,
    idempotency_key: &str,
) -> Result<(Vec<Operation>, Value), storycut_core::CoreError> {
    use storycut_core::{AudioClip, AudioSettings, ImageClip, VideoClip};
    let fail = |message: String| storycut_core::CoreError::validation(message);
    let items = args
        .get("items")
        .and_then(Value::as_array)
        .filter(|items| !items.is_empty() && items.len() <= 200)
        .ok_or_else(|| fail("items must contain 1 to 200 assets".into()))?;
    let frame_ticks = project
        .frame_ticks()
        .ok_or_else(|| fail("project frame rate cannot be represented by the timebase".into()))?;
    let transition = parse_transition_policy(args.get("transition"))?;
    let (transition_requested_ticks, transition_ticks) = match transition.mode {
        TransitionMode::CrossDissolve => {
            let requested = seconds_value_to_ticks(
                transition.duration_seconds.as_ref().unwrap_or(&json!(1)),
                "transition.duration_seconds",
                false,
            )?;
            (requested, round_to_grid(requested, frame_ticks)?)
        }
        _ => (0, 0),
    };
    if transition.mode == TransitionMode::CrossDissolve && transition_ticks == 0 {
        return Err(fail("cross dissolve duration rounds to zero frames".into()));
    }
    let original_video_audio = args
        .get("original_video_audio")
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| fail("original_video_audio must be a string".into()))
        })
        .transpose()?
        .unwrap_or("separate_linked");
    let original_audio = match original_video_audio {
        "separate_linked" => true,
        "muted" => false,
        _ => {
            return Err(fail(
                "original_video_audio must be separate_linked or muted".into(),
            ));
        }
    };

    let mut operations = Vec::new();
    let mut created_track_ids = Vec::<String>::new();
    let mut created_clip_ids = Vec::<String>::new();
    let mut track_roles = Vec::<Value>::new();
    let mut warnings = Vec::<Value>::new();
    let visual_track_id = if let Some(track_id) = args.get("visual_track_id") {
        let track_id = track_id
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or_else(|| fail("visual_track_id must be a nonempty string".into()))?;
        let track = project
            .tracks
            .iter()
            .find(|track| track.id == track_id)
            .ok_or_else(|| fail(format!("visual track {track_id} was not found")))?;
        if track.kind != TrackKind::Video || track.locked {
            return Err(fail(format!(
                "visual track {track_id} must be an unlocked video track"
            )));
        }
        track_id.to_owned()
    } else {
        let track_id = generated_id(project, idempotency_key, "story-video-track", 0);
        operations.push(Operation::TrackAdd {
            track: make_track(track_id.clone(), "故事畫面", TrackKind::Video),
        });
        created_track_ids.push(track_id.clone());
        track_roles.push(json!({"role":"visual","track_id":track_id,"name":"故事畫面"}));
        track_id
    };
    let mut visual_cursor = project
        .clips
        .iter()
        .filter(|clip| clip.track_id() == visual_track_id)
        .filter_map(Clip::end_tick)
        .max()
        .unwrap_or(0);
    let assembly_start_tick = visual_cursor;
    let mut previous_visual: Option<(String, u64, u64)> = None;
    let mut original_audio_track: Option<String> = None;
    let mut audio_links = Vec::<Operation>::new();

    for (index, item) in items.iter().enumerate() {
        require_object_keys(item, &["asset_id", "source_in_seconds", "duration_seconds"])
            .map_err(command_to_core)?;
        let asset_id = item
            .get("asset_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| fail(format!("items[{index}].asset_id is required")))?;
        let asset = project
            .assets
            .iter()
            .find(|asset| asset.id == asset_id)
            .ok_or_else(|| fail(format!("asset {asset_id} was not found")))?;
        if !matches!(asset.kind, AssetKind::Image | AssetKind::Video) {
            return Err(fail(format!(
                "asset {asset_id} is not a visual image or video"
            )));
        }
        let source_in = item
            .get("source_in_seconds")
            .map(|value| seconds_value_to_ticks(value, "source_in_seconds", true))
            .transpose()?
            .unwrap_or(0);
        let source_in = if asset.kind == AssetKind::Video {
            round_to_grid(source_in, frame_ticks)?
        } else {
            if source_in != 0 {
                return Err(fail(format!(
                    "image asset {asset_id} cannot have a source in point"
                )));
            }
            0
        };
        let duration_is_explicit = item.get("duration_seconds").is_some();
        let duration_raw = if let Some(value) = item.get("duration_seconds") {
            seconds_value_to_ticks(value, "duration_seconds", false)?
        } else if asset.kind == AssetKind::Image {
            10 * TIMEBASE
        } else {
            let total = asset.duration_ticks.ok_or_else(|| {
                fail(format!(
                    "video asset {asset_id} has no probed natural duration"
                ))
            })?;
            total.checked_sub(source_in).ok_or_else(|| {
                fail(format!(
                    "video asset {asset_id} source in exceeds its duration"
                ))
            })?
        };
        let duration = if asset.kind == AssetKind::Video && !duration_is_explicit {
            floor_to_grid(duration_raw, frame_ticks)?
        } else {
            round_to_grid(duration_raw, frame_ticks)?
        };
        if duration == 0 {
            return Err(fail(format!(
                "items[{index}] duration rounds to zero frames"
            )));
        }
        if asset.kind == AssetKind::Video {
            let total = asset.duration_ticks.ok_or_else(|| {
                fail(format!(
                    "video asset {asset_id} has no probed natural duration"
                ))
            })?;
            if source_in
                .checked_add(duration)
                .is_none_or(|end| end > total)
            {
                return Err(fail(format!(
                    "video asset {asset_id} source range exceeds its duration"
                )));
            }
        }
        let clip_id = generated_id(project, idempotency_key, "story-visual-clip", index);
        let motion = default_motion(duration, frame_ticks);
        let clip = match asset.kind {
            AssetKind::Image => Clip::Image(ImageClip {
                id: clip_id.clone(),
                track_id: visual_track_id.clone(),
                asset_id: asset_id.to_owned(),
                start_tick: visual_cursor,
                duration_ticks: duration,
                source_in_tick: 0,
                motion,
            }),
            AssetKind::Video => {
                let video_stream = asset
                    .streams
                    .iter()
                    .find(|stream| stream.kind == StreamKind::Video)
                    .ok_or_else(|| fail(format!("video asset {asset_id} has no video stream")))?;
                let video = VideoClip {
                    id: clip_id.clone(),
                    track_id: visual_track_id.clone(),
                    asset_id: asset_id.to_owned(),
                    start_tick: visual_cursor,
                    duration_ticks: duration,
                    source_in_tick: source_in,
                    stream_index: video_stream.index,
                    motion,
                    audio_policy: VideoAudioPolicy::Muted,
                    hold_head_ticks: 0,
                    hold_tail_ticks: 0,
                };
                if original_audio
                    && asset
                        .streams
                        .iter()
                        .any(|stream| stream.kind == StreamKind::Audio)
                {
                    let audio_stream = asset
                        .streams
                        .iter()
                        .find(|stream| stream.kind == StreamKind::Audio)
                        .expect("audio stream existence checked");
                    let track_id = original_audio_track.get_or_insert_with(|| {
                        let id =
                            generated_id(project, idempotency_key, "story-original-audio-track", 0);
                        operations.push(Operation::TrackAdd {
                            track: make_track(id.clone(), "原始影片聲音", TrackKind::Audio),
                        });
                        created_track_ids.push(id.clone());
                        track_roles.push(
                            json!({"role":"original_audio","track_id":id,"name":"原始影片聲音"}),
                        );
                        id
                    });
                    let audio_id =
                        generated_id(project, idempotency_key, "story-original-audio-clip", index);
                    let audio = AudioClip {
                        id: audio_id.clone(),
                        track_id: track_id.clone(),
                        asset_id: asset_id.to_owned(),
                        start_tick: visual_cursor,
                        duration_ticks: duration,
                        source_in_tick: source_in,
                        stream_index: audio_stream.index,
                        audio: AudioSettings {
                            domain_duration_ticks: duration,
                            sample_offset_tick: 0,
                            gain_db: 0.0,
                            pan: 0.0,
                            muted: false,
                            fade_in_ticks: 0,
                            fade_out_ticks: 0,
                            fade_curve: "linear_amplitude".into(),
                        },
                    };
                    operations.push(Operation::ClipAdd {
                        clip: Clip::Video(video.clone()),
                    });
                    operations.push(Operation::ClipAdd {
                        clip: Clip::Audio(audio),
                    });
                    created_clip_ids.push(audio_id.clone());
                    audio_links.push(Operation::LinkCreate {
                        link: Link {
                            id: generated_id(
                                project,
                                idempotency_key,
                                "story-original-audio-link",
                                index,
                            ),
                            kind: "av_sync".into(),
                            clip_ids: [video.id.clone(), audio_id],
                        },
                    });
                } else {
                    operations.push(Operation::ClipAdd {
                        clip: Clip::Video(video),
                    });
                }
                // This placeholder is replaced below only to share the common clip ID flow.
                Clip::Video(VideoClip {
                    id: clip_id.clone(),
                    track_id: visual_track_id.clone(),
                    asset_id: asset_id.to_owned(),
                    start_tick: visual_cursor,
                    duration_ticks: duration,
                    source_in_tick: source_in,
                    stream_index: video_stream.index,
                    motion: default_motion(duration, frame_ticks),
                    audio_policy: VideoAudioPolicy::Muted,
                    hold_head_ticks: 0,
                    hold_tail_ticks: 0,
                })
            }
            AssetKind::Audio => unreachable!("visual kinds checked above"),
        };
        if asset.kind == AssetKind::Image {
            operations.push(Operation::ClipAdd { clip: clip.clone() });
        }
        created_clip_ids.push(clip_id.clone());
        if let Some((from_id, from_start, from_end)) = previous_visual {
            if transition.mode == TransitionMode::CrossDissolve {
                let prev_duration = from_end.saturating_sub(from_start);
                if transition_ticks > prev_duration || transition_ticks > duration {
                    return Err(fail(format!(
                        "cross dissolve between {from_id} and {clip_id} exceeds a clip duration"
                    )));
                }
                operations.push(Operation::TransitionSet {
                    transition: Transition {
                        id: generated_id(project, idempotency_key, "story-transition", index - 1),
                        track_id: visual_track_id.clone(),
                        from_clip_id: from_id,
                        to_clip_id: clip_id.clone(),
                        start_tick: visual_cursor,
                        duration_ticks: transition_ticks,
                        kind: TransitionKind::CrossDissolve,
                        curve: "linear".into(),
                        audio_policy: "independent".into(),
                    },
                });
            }
        }
        let visual_end = visual_cursor
            .checked_add(duration)
            .filter(|end| *end <= MAX_TICKS)
            .ok_or_else(|| fail("storyboard timeline range overflows".into()))?;
        previous_visual = Some((clip_id, visual_cursor, visual_end));
        visual_cursor = if transition.mode == TransitionMode::CrossDissolve {
            visual_end
                .checked_sub(transition_ticks)
                .ok_or_else(|| fail("storyboard overlap exceeds the clip duration".into()))?
        } else {
            visual_end
        };
    }
    // The final clip ends at the current cursor plus the last overlap.
    let visual_end_tick = previous_visual
        .map(|(_, _, end)| end)
        .unwrap_or(visual_cursor);

    let audio_tracks = match args.get("audio_tracks") {
        None => Vec::new(),
        Some(value) => value
            .as_array()
            .cloned()
            .ok_or_else(|| fail("audio_tracks must be an array".into()))?,
    };
    if audio_tracks.len() > 32 {
        return Err(fail("audio_tracks may contain at most 32 lanes".into()));
    }
    let mut audio_end_tick = 0_u64;
    for (lane_index, lane) in audio_tracks.iter().enumerate() {
        require_object_keys(lane, &["role", "track_id", "track_name", "clips"])
            .map_err(command_to_core)?;
        let role = lane
            .get("role")
            .and_then(Value::as_str)
            .filter(|role| matches!(*role, "narration" | "dialogue" | "music"))
            .ok_or_else(|| fail(format!("audio_tracks[{lane_index}].role is invalid")))?;
        let default_name = match role {
            "narration" => "旁白",
            "dialogue" => "角色對白",
            "music" => "配樂",
            _ => unreachable!(),
        };
        let name = match lane.get("track_name") {
            None => default_name,
            Some(value) => value
                .as_str()
                .filter(|name| !name.trim().is_empty())
                .ok_or_else(|| {
                    fail(format!(
                        "audio_tracks[{lane_index}].track_name must be a nonempty string"
                    ))
                })?,
        };
        let lane_track_id = if let Some(track_id) = lane.get("track_id") {
            let track_id = track_id
                .as_str()
                .filter(|id| !id.is_empty())
                .ok_or_else(|| fail(format!("audio_tracks[{lane_index}].track_id is invalid")))?;
            let track = project
                .tracks
                .iter()
                .find(|track| track.id == track_id)
                .ok_or_else(|| fail(format!("audio track {track_id} was not found")))?;
            if track.kind != TrackKind::Audio || track.locked {
                return Err(fail(format!(
                    "audio track {track_id} must be unlocked and kind audio"
                )));
            }
            if lane.get("track_name").is_some() && name != track.name {
                operations.push(Operation::TrackUpdate {
                    track_id: track_id.to_owned(),
                    changes: storycut_core::TrackChanges {
                        name: Some(name.to_owned()),
                        ..Default::default()
                    },
                });
            }
            track_id.to_owned()
        } else {
            let id = generated_id(project, idempotency_key, "story-role-track", lane_index);
            operations.push(Operation::TrackAdd {
                track: make_track(id.clone(), name, TrackKind::Audio),
            });
            created_track_ids.push(id.clone());
            track_roles.push(json!({"role":role,"track_id":id,"name":name}));
            id
        };
        let clips = lane
            .get("clips")
            .and_then(Value::as_array)
            .filter(|clips| !clips.is_empty() && clips.len() <= 200)
            .ok_or_else(|| {
                fail(format!(
                    "audio_tracks[{lane_index}].clips must contain 1 to 200 items"
                ))
            })?;
        for (clip_index, audio_item) in clips.iter().enumerate() {
            require_object_keys(
                audio_item,
                &[
                    "asset_id",
                    "start_seconds",
                    "source_in_seconds",
                    "duration_seconds",
                    "gain_db",
                    "fade_in_seconds",
                    "fade_out_seconds",
                    "loop_to_visual_end",
                ],
            )
            .map_err(command_to_core)?;
            let asset_id = audio_item
                .get("asset_id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .ok_or_else(|| {
                    fail(format!(
                        "audio clip {lane_index}:{clip_index} needs asset_id"
                    ))
                })?;
            let asset = project
                .assets
                .iter()
                .find(|asset| asset.id == asset_id)
                .ok_or_else(|| fail(format!("audio asset {asset_id} was not found")))?;
            if asset.kind == AssetKind::Image {
                return Err(fail(format!("asset {asset_id} is not an audio source")));
            }
            let audio_stream = asset
                .streams
                .iter()
                .find(|stream| stream.kind == StreamKind::Audio)
                .ok_or_else(|| fail(format!("asset {asset_id} has no audio stream")))?;
            let start_ticks_raw = seconds_value_to_ticks(
                audio_item
                    .get("start_seconds")
                    .ok_or_else(|| fail("audio clip needs start_seconds".into()))?,
                "start_seconds",
                true,
            )?;
            let sample_ticks = TIMEBASE / u64::from(project.audio_sample_rate);
            if sample_ticks == 0 || TIMEBASE % u64::from(project.audio_sample_rate) != 0 {
                return Err(fail(
                    "project audio sample rate is not representable by timebase".into(),
                ));
            }
            let start_ticks = round_to_grid(start_ticks_raw, sample_ticks)?;
            let source_in = audio_item
                .get("source_in_seconds")
                .map(|value| seconds_value_to_ticks(value, "source_in_seconds", true))
                .transpose()?
                .unwrap_or(0);
            let source_total = asset
                .duration_ticks
                .ok_or_else(|| fail(format!("audio asset {asset_id} has no probed duration")))?;
            if source_in >= source_total {
                return Err(fail(format!(
                    "audio asset {asset_id} source in is past its duration"
                )));
            }
            let default_source_duration = source_total - source_in;
            let source_duration_raw = if let Some(value) = audio_item.get("duration_seconds") {
                seconds_value_to_ticks(value, "duration_seconds", false)?
            } else {
                default_source_duration
            };
            let source_duration = round_to_grid(source_duration_raw, sample_ticks)?;
            if source_duration == 0
                || source_in
                    .checked_add(source_duration)
                    .is_none_or(|end| end > source_total)
            {
                return Err(fail(format!(
                    "audio asset {asset_id} source range exceeds its duration"
                )));
            }
            let loop_to_end = audio_item
                .get("loop_to_visual_end")
                .map(|value| {
                    value
                        .as_bool()
                        .ok_or_else(|| fail("loop_to_visual_end must be a boolean".into()))
                })
                .transpose()?
                .unwrap_or(false);
            if loop_to_end && role != "music" {
                return Err(fail(
                    "loop_to_visual_end is supported only for music".into(),
                ));
            }
            let gain_db = finite_number(audio_item.get("gain_db"), 0.0, "gain_db")?;
            if !gain_db.is_finite() || !(-96.0..=24.0).contains(&gain_db) {
                return Err(fail("gain_db must be within -96..=24".into()));
            }
            let fade_in_ticks = audio_item
                .get("fade_in_seconds")
                .map(|value| seconds_value_to_ticks(value, "fade_in_seconds", true))
                .transpose()?
                .map(|ticks| round_to_grid(ticks, sample_ticks))
                .transpose()?
                .unwrap_or(0);
            let fade_out_ticks = audio_item
                .get("fade_out_seconds")
                .map(|value| seconds_value_to_ticks(value, "fade_out_seconds", true))
                .transpose()?
                .map(|ticks| round_to_grid(ticks, sample_ticks))
                .transpose()?
                .unwrap_or(0);
            let mut timeline_cursor = start_ticks;
            let mut loop_parts = Vec::<(u64, u64)>::new();
            if loop_to_end {
                if timeline_cursor >= visual_end_tick {
                    return Err(fail(
                        "looping music must start before the storyboard visual end".into(),
                    ));
                }
                while timeline_cursor < visual_end_tick {
                    if loop_parts.len() >= 500 {
                        return Err(fail(
                            "music loop would exceed the 500-operation transaction limit".into(),
                        ));
                    }
                    let left = visual_end_tick - timeline_cursor;
                    let duration = if left <= source_duration {
                        // Keep the loop inside the storyboard end when the final
                        // video-frame boundary falls between audio samples.
                        floor_to_grid(left, sample_ticks)?
                    } else {
                        source_duration
                    };
                    if duration == 0 {
                        break;
                    }
                    loop_parts.push((timeline_cursor, duration));
                    timeline_cursor = timeline_cursor
                        .checked_add(duration)
                        .ok_or_else(|| fail("audio timeline range overflows".into()))?;
                }
            } else {
                loop_parts.push((timeline_cursor, source_duration));
            }
            for (part_index, (part_start, part_duration)) in loop_parts.iter().copied().enumerate()
            {
                let first = part_index == 0;
                let last = part_index + 1 == loop_parts.len();
                let part_fade_in = if first { fade_in_ticks } else { 0 };
                let part_fade_out = if last { fade_out_ticks } else { 0 };
                if part_fade_in.saturating_add(part_fade_out) > part_duration {
                    return Err(fail(format!(
                        "audio fades exceed the edge loop segment at lane {lane_index}, clip {clip_index}"
                    )));
                }
                // Each lane owns a disjoint 100,000-index block. Up to 200
                // clips × 500 loop parts fills at most 100,000 indices.
                let generated_index = lane_index * 100_000 + clip_index * 500 + part_index;
                let clip_id = generated_id(
                    project,
                    idempotency_key,
                    "story-role-audio-clip",
                    generated_index,
                );
                let clip = AudioClip {
                    id: clip_id.clone(),
                    track_id: lane_track_id.clone(),
                    asset_id: asset_id.to_owned(),
                    start_tick: part_start,
                    duration_ticks: part_duration,
                    source_in_tick: source_in,
                    stream_index: audio_stream.index,
                    audio: AudioSettings {
                        domain_duration_ticks: part_duration,
                        sample_offset_tick: 0,
                        gain_db,
                        pan: 0.0,
                        muted: false,
                        fade_in_ticks: part_fade_in,
                        fade_out_ticks: part_fade_out,
                        fade_curve: "linear_amplitude".into(),
                    },
                };
                operations.push(Operation::ClipAdd {
                    clip: Clip::Audio(clip),
                });
                created_clip_ids.push(clip_id);
                audio_end_tick = audio_end_tick.max(part_start.saturating_add(part_duration));
            }
        }
    }
    operations.extend(audio_links);
    if operations.is_empty() {
        return Err(fail("storyboard plan contains no operations".into()));
    }
    if operations.len() > 500 {
        return Err(fail(format!(
            "storyboard expands to {} operations; the transaction limit is 500",
            operations.len()
        )));
    }
    let visual_duration_ticks = visual_end_tick.saturating_sub(assembly_start_tick);
    let audio_duration_ticks = audio_end_tick.saturating_sub(assembly_start_tick);
    if transition_ticks > 0 && transition_requested_ticks != transition_ticks {
        warnings.push(json!({
            "code":"TRANSITION_ROUNDED_TO_FRAME",
            "requested_duration_ticks":transition_requested_ticks,
            "duration_ticks":transition_ticks
        }));
    }
    let response_data = json!({
        "created_track_ids":created_track_ids,
        "created_clip_ids":created_clip_ids,
        "visual_track_id":visual_track_id,
        "start_tick":assembly_start_tick,
        "visual_duration_ticks":visual_duration_ticks,
        "audio_duration_ticks":audio_duration_ticks,
        "track_roles":track_roles,
        "transition":{"kind":if transition.mode == TransitionMode::CrossDissolve {"cross_dissolve"} else {"cut"},"duration_ticks":transition_ticks},
        "warnings":warnings,
    });
    Ok((operations, response_data))
}

#[derive(Clone)]
struct TransitionPolicy {
    mode: TransitionMode,
    duration_seconds: Option<Value>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TransitionMode {
    Cut,
    CrossDissolve,
}

fn parse_transition_policy(
    value: Option<&Value>,
) -> Result<TransitionPolicy, storycut_core::CoreError> {
    let Some(value) = value else {
        return Ok(TransitionPolicy {
            mode: TransitionMode::Cut,
            duration_seconds: None,
        });
    };
    require_object_keys(value, &["kind", "duration_seconds"]).map_err(command_to_core)?;
    let kind_value = value
        .get("kind")
        .map(|value| {
            value.as_str().ok_or_else(|| {
                storycut_core::CoreError::validation("transition.kind must be a string")
            })
        })
        .transpose()?
        .unwrap_or("cut");
    let kind = match kind_value {
        "cut" => TransitionMode::Cut,
        "cross_dissolve" => TransitionMode::CrossDissolve,
        other => {
            return Err(storycut_core::CoreError::validation(format!(
                "unsupported transition kind {other}"
            )));
        }
    };
    let duration_seconds = if kind == TransitionMode::CrossDissolve {
        Some(
            value
                .get("duration_seconds")
                .cloned()
                .unwrap_or_else(|| json!(1)),
        )
    } else {
        if value.get("duration_seconds").is_some() {
            return Err(storycut_core::CoreError::validation(
                "transition.duration_seconds is only valid for cross_dissolve",
            ));
        }
        None
    };
    Ok(TransitionPolicy {
        mode: kind,
        duration_seconds,
    })
}

fn plan_focal_motion(
    workspace: &Path,
    project: &Project,
    args: &Value,
) -> Result<(Vec<Operation>, Value), storycut_core::CoreError> {
    let fail = |message: String| storycut_core::CoreError::validation(message);
    let targets = args
        .get("targets")
        .and_then(Value::as_array)
        .filter(|targets| !targets.is_empty() && targets.len() <= 200)
        .ok_or_else(|| fail("targets must contain 1 to 200 clips".into()))?;
    let preset = args
        .get("preset")
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| fail("preset must be a string".into()))
        })
        .transpose()?
        .unwrap_or("focus_zoom");
    let (default_from, default_to) = match preset {
        "focus_zoom" | "zoom_in" => (1.0, 1.18),
        "zoom_out" => (1.18, 1.0),
        "static" => (1.0, 1.0),
        "pan_left" | "pan_right" | "pan_up" | "pan_down" => (1.1, 1.1),
        _ => return Err(fail(format!("unsupported focal motion preset {preset}"))),
    };
    let from_scale = finite_number(args.get("from_scale"), default_from, "from_scale")?;
    let to_scale = finite_number(args.get("to_scale"), default_to, "to_scale")?;
    if !(1.0..=16.0).contains(&from_scale) || !(1.0..=16.0).contains(&to_scale) {
        return Err(fail("from_scale and to_scale must be within 1..=16".into()));
    }
    let pan_amount = finite_number(args.get("pan_amount"), 0.03, "pan_amount")?;
    if !(0.0..=0.5).contains(&pan_amount) {
        return Err(fail("pan_amount must be within 0..=0.5".into()));
    }
    let mut operations = Vec::with_capacity(targets.len());
    let mut warnings = Vec::<Value>::new();
    let mut changed_clip_ids = Vec::<String>::new();
    let mut seen = std::collections::HashSet::<String>::new();
    let frame_ticks = project
        .frame_ticks()
        .ok_or_else(|| fail("project frame rate cannot be represented by the timebase".into()))?;
    for (index, target) in targets.iter().enumerate() {
        require_object_keys(target, &["clip_id", "focus"]).map_err(command_to_core)?;
        let clip_id = target
            .get("clip_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| fail(format!("targets[{index}].clip_id is required")))?;
        if !seen.insert(clip_id.to_owned()) {
            return Err(fail(format!("clip {clip_id} appears more than once")));
        }
        let clip = project
            .clips
            .iter()
            .find(|clip| clip.id() == clip_id)
            .ok_or_else(|| fail(format!("clip {clip_id} was not found")))?;
        if !clip.is_visual() {
            return Err(fail(format!("clip {clip_id} is not a visual clip")));
        }
        let asset = project
            .assets
            .iter()
            .find(|asset| asset.id == clip.asset_id())
            .ok_or_else(|| fail(format!("asset for clip {clip_id} was not found")))?;
        let media_path = checked_path(workspace, &asset.path, true).map_err(command_to_core)?;
        let probe = storycut_render::probe_media(&media_path)
            .map_err(render_error)
            .map_err(command_to_core)?;
        if asset
            .sha256
            .as_deref()
            .is_none_or(|hash| !hash.eq_ignore_ascii_case(&probe.sha256))
        {
            return Err(storycut_core::CoreError::new(
                storycut_core::CoreErrorCode::MediaChanged,
                format!("asset {} changed after it was imported", asset.id),
            ));
        }
        let source_width = probe.width.filter(|width| *width > 0).ok_or_else(|| {
            storycut_core::CoreError::new(
                storycut_core::CoreErrorCode::UnsupportedFeature,
                format!("asset {} has no usable video width", asset.id),
            )
        })?;
        let source_height = probe.height.filter(|height| *height > 0).ok_or_else(|| {
            storycut_core::CoreError::new(
                storycut_core::CoreErrorCode::UnsupportedFeature,
                format!("asset {} has no usable video height", asset.id),
            )
        })?;
        let focus = if let Some(focus) = target.get("focus") {
            require_object_keys(focus, &["x", "y"]).map_err(command_to_core)?;
            let x = finite_number(focus.get("x"), f64::NAN, "focus.x")?;
            let y = finite_number(focus.get("y"), f64::NAN, "focus.y")?;
            if !(0.0..=1.0).contains(&x) || !(0.0..=1.0).contains(&y) {
                return Err(fail(format!(
                    "focus for clip {clip_id} must be normalized within 0..=1"
                )));
            }
            Anchor { x, y }
        } else {
            Anchor { x: 0.5, y: 0.5 }
        };
        let canvas = &project.canvas;
        let pan_start = pan_vector(preset, pan_amount, true);
        let pan_end = pan_vector(preset, pan_amount, false);
        let (first_x, first_y, first_clamped) = focal_position(
            canvas.width,
            canvas.height,
            source_width,
            source_height,
            from_scale,
            &focus,
            pan_start.0,
            pan_start.1,
        );
        let (last_x, last_y, last_clamped) = focal_position(
            canvas.width,
            canvas.height,
            source_width,
            source_height,
            to_scale,
            &focus,
            pan_end.0,
            pan_end.1,
        );
        if let Some((actual_x, actual_y)) = first_clamped {
            warnings.push(json!({
                "code":if preset.starts_with("pan_") {"PAN_CLAMPED"} else {"FOCUS_CLAMPED"},
                "message":"取景已限制在圖片邊界內，以避免露出空白。",
                "clip_id":clip_id,
                "keyframe":"start",
                "requested_focus":{"x":focus.x,"y":focus.y},
                "actual_focus":{"x":actual_x,"y":actual_y}
            }));
        }
        if let Some((actual_x, actual_y)) = last_clamped {
            warnings.push(json!({
                "code":if preset.starts_with("pan_") {"PAN_CLAMPED"} else {"FOCUS_CLAMPED"},
                "message":"取景已限制在圖片邊界內，以避免露出空白。",
                "clip_id":clip_id,
                "keyframe":"end",
                "requested_focus":{"x":focus.x,"y":focus.y},
                "actual_focus":{"x":actual_x,"y":actual_y}
            }));
        }
        let visible_duration = clip.duration_ticks();
        if visible_duration == 0 || visible_duration % frame_ticks != 0 {
            return Err(fail(format!(
                "clip {clip_id} duration is not frame aligned"
            )));
        }
        let end_key_tick = visible_duration - frame_ticks;
        let first = Keyframe {
            tick: 0,
            x: first_x,
            y: first_y,
            scale: from_scale,
            opacity: 1.0,
        };
        let keyframes = if visible_duration == frame_ticks {
            vec![first]
        } else {
            vec![
                first,
                Keyframe {
                    tick: end_key_tick,
                    x: last_x,
                    y: last_y,
                    scale: to_scale,
                    opacity: 1.0,
                },
            ]
        };
        let motion = Motion {
            domain_duration_ticks: visible_duration,
            sample_offset_tick: 0,
            fit: FitMode::Cover,
            avoid_exposed_edges: true,
            anchor: Anchor { x: 0.5, y: 0.5 },
            interpolation: Interpolation::Smoothstep,
            keyframes,
        };
        operations.push(Operation::MotionSet {
            clip_id: clip_id.to_owned(),
            motion,
        });
        changed_clip_ids.push(clip_id.to_owned());
    }
    let data = json!({
        "changed_clip_ids":changed_clip_ids,
        "preset":preset,
        "from_scale":from_scale,
        "to_scale":to_scale,
        "pan_amount":pan_amount,
        "warnings":warnings,
    });
    Ok((operations, data))
}

fn make_track(id: String, name: &str, kind: TrackKind) -> Track {
    Track {
        id,
        name: name.to_owned(),
        kind,
        locked: false,
        enabled: true,
        muted: false,
        solo: false,
        gain_db: 0.0,
        ducking: None,
    }
}

fn generated_id(project: &Project, key: &str, purpose: &str, index: usize) -> String {
    let mut hash = Sha256::new();
    hash.update(project.project_id.as_bytes());
    hash.update(b"\0");
    hash.update(key.as_bytes());
    hash.update(b"\0");
    hash.update(purpose.as_bytes());
    hash.update(b"\0");
    hash.update(index.to_le_bytes());
    format!(
        "sc-{}-{}",
        &purpose.replace("story-", ""),
        &format!("{:x}", hash.finalize())[..20]
    )
}

fn default_motion(duration_ticks: u64, frame_ticks: u64) -> Motion {
    let end = duration_ticks.saturating_sub(frame_ticks);
    let keyframes = if duration_ticks == frame_ticks {
        vec![Keyframe {
            tick: 0,
            x: 0.0,
            y: 0.0,
            scale: 1.0,
            opacity: 1.0,
        }]
    } else {
        vec![
            Keyframe {
                tick: 0,
                x: 0.0,
                y: 0.0,
                scale: 1.0,
                opacity: 1.0,
            },
            Keyframe {
                tick: end,
                x: 0.0,
                y: 0.0,
                scale: 1.0,
                opacity: 1.0,
            },
        ]
    };
    Motion {
        domain_duration_ticks: duration_ticks,
        sample_offset_tick: 0,
        fit: FitMode::Cover,
        avoid_exposed_edges: true,
        anchor: Anchor { x: 0.5, y: 0.5 },
        interpolation: Interpolation::Smoothstep,
        keyframes,
    }
}

fn seconds_value_to_ticks(
    value: &Value,
    field: &str,
    allow_zero: bool,
) -> Result<u64, storycut_core::CoreError> {
    let number = value
        .as_number()
        .ok_or_else(|| storycut_core::CoreError::validation(format!("{field} must be a number")))?;
    let raw = number.to_string();
    let invalid = || {
        storycut_core::CoreError::validation(format!(
            "{field} must be a finite nonnegative decimal"
        ))
    };
    let (mantissa, exponent) = match raw.find(['e', 'E']) {
        Some(index) => (
            &raw[..index],
            raw[index + 1..].parse::<i32>().map_err(|_| invalid())?,
        ),
        None => (raw.as_str(), 0),
    };
    if mantissa.starts_with('-') {
        return Err(invalid());
    }
    let mantissa = mantissa.strip_prefix('+').unwrap_or(mantissa);
    let mut digits = String::with_capacity(mantissa.len());
    let mut fraction_len: i32 = 0;
    let mut after_point = false;
    for ch in mantissa.chars() {
        match ch {
            '0'..='9' => {
                digits.push(ch);
                if after_point {
                    fraction_len += 1;
                }
            }
            '.' if !after_point => after_point = true,
            _ => return Err(invalid()),
        }
    }
    if digits.is_empty() {
        return Err(invalid());
    }
    let digits = digits.parse::<u128>().map_err(|_| invalid())?;
    let scale = fraction_len - exponent;
    let numerator = digits
        .checked_mul(u128::from(TIMEBASE))
        .ok_or_else(invalid)?;
    let ticks = if scale <= 0 {
        let multiplier =
            pow10(u32::try_from(-scale).map_err(|_| invalid())?).ok_or_else(invalid)?;
        numerator.checked_mul(multiplier).ok_or_else(invalid)?
    } else {
        let divisor = pow10(u32::try_from(scale).map_err(|_| invalid())?).ok_or_else(invalid)?;
        round_ratio_ties_even(numerator, divisor).ok_or_else(invalid)?
    };
    let ticks = u64::try_from(ticks).map_err(|_| invalid())?;
    if ticks > MAX_TICKS || (!allow_zero && ticks == 0) {
        return Err(invalid());
    }
    Ok(ticks)
}

fn pow10(exponent: u32) -> Option<u128> {
    (0..exponent).try_fold(1_u128, |value, _| value.checked_mul(10))
}

fn round_ratio_ties_even(numerator: u128, denominator: u128) -> Option<u128> {
    if denominator == 0 {
        return None;
    }
    let quotient = numerator / denominator;
    let remainder = numerator % denominator;
    match remainder.checked_mul(2)?.cmp(&denominator) {
        std::cmp::Ordering::Less => Some(quotient),
        std::cmp::Ordering::Greater => quotient.checked_add(1),
        std::cmp::Ordering::Equal if quotient % 2 == 0 => Some(quotient),
        std::cmp::Ordering::Equal => quotient.checked_add(1),
    }
}

fn round_to_grid(ticks: u64, grid: u64) -> Result<u64, storycut_core::CoreError> {
    if grid == 0 {
        return Err(storycut_core::CoreError::validation(
            "time grid must be positive",
        ));
    }
    let rounded = round_ratio_ties_even(u128::from(ticks), u128::from(grid))
        .and_then(|count| count.checked_mul(u128::from(grid)))
        .and_then(|value| u64::try_from(value).ok())
        .filter(|value| *value <= MAX_TICKS)
        .ok_or_else(|| {
            storycut_core::CoreError::validation("time value exceeds the allowed range")
        })?;
    Ok(rounded)
}

fn floor_to_grid(ticks: u64, grid: u64) -> Result<u64, storycut_core::CoreError> {
    if grid == 0 {
        return Err(storycut_core::CoreError::validation(
            "time grid must be positive",
        ));
    }
    Ok((ticks / grid) * grid)
}

fn finite_number(
    value: Option<&Value>,
    default: f64,
    field: &str,
) -> Result<f64, storycut_core::CoreError> {
    let value = match value {
        Some(value) => value.as_f64().ok_or_else(|| {
            storycut_core::CoreError::validation(format!("{field} must be a number"))
        })?,
        None => default,
    };
    if !value.is_finite() {
        return Err(storycut_core::CoreError::validation(format!(
            "{field} must be finite"
        )));
    }
    Ok(value)
}

fn pan_vector(preset: &str, amount: f64, start: bool) -> (f64, f64) {
    let sign = if start { 1.0 } else { -1.0 };
    match preset {
        "pan_left" => (sign * amount, 0.0),
        "pan_right" => (-sign * amount, 0.0),
        "pan_up" => (0.0, sign * amount),
        "pan_down" => (0.0, -sign * amount),
        _ => (0.0, 0.0),
    }
}

fn focal_position(
    canvas_width: u32,
    canvas_height: u32,
    source_width: u32,
    source_height: u32,
    motion_scale: f64,
    focus: &Anchor,
    extra_x: f64,
    extra_y: f64,
) -> (f64, f64, Option<(f64, f64)>) {
    let canvas_width = f64::from(canvas_width);
    let canvas_height = f64::from(canvas_height);
    let fit =
        (canvas_width / f64::from(source_width)).max(canvas_height / f64::from(source_height));
    let scaled_width = f64::from(source_width) * fit * motion_scale;
    let scaled_height = f64::from(source_height) * fit * motion_scale;
    let base_x = (canvas_width - scaled_width) * 0.5;
    let base_y = (canvas_height - scaled_height) * 0.5;
    let desired_x = (canvas_width * 0.5 - focus.x * scaled_width - base_x) / canvas_width + extra_x;
    let desired_y =
        (canvas_height * 0.5 - focus.y * scaled_height - base_y) / canvas_height + extra_y;
    let min_x = (canvas_width - scaled_width - base_x) / canvas_width;
    let max_x = -base_x / canvas_width;
    let min_y = (canvas_height - scaled_height - base_y) / canvas_height;
    let max_y = -base_y / canvas_height;
    let x = desired_x.clamp(min_x, max_x);
    let y = desired_y.clamp(min_y, max_y);
    let clamped = if (x - desired_x).abs() > 1e-9 || (y - desired_y).abs() > 1e-9 {
        Some((
            (base_x + x * canvas_width + focus.x * scaled_width) / canvas_width,
            (base_y + y * canvas_height + focus.y * scaled_height) / canvas_height,
        ))
    } else {
        None
    };
    (x, y, clamped)
}

fn command_to_core(error: CommandError) -> storycut_core::CoreError {
    use storycut_core::CoreErrorCode;
    let code = match error.code.as_str() {
        "NOT_FOUND" => CoreErrorCode::NotFound,
        "PATH_DENIED" => CoreErrorCode::PathDenied,
        "UNSUPPORTED_FEATURE" => CoreErrorCode::UnsupportedFeature,
        "MEDIA_CHANGED" => CoreErrorCode::MediaChanged,
        "LOCKED_TRACK" => CoreErrorCode::LockedTrack,
        "REVISION_CONFLICT" => CoreErrorCode::RevisionConflict,
        "IDEMPOTENCY_CONFLICT" => CoreErrorCode::IdempotencyConflict,
        "DEPENDENCY_CONFLICT" => CoreErrorCode::Conflict,
        "INTERNAL_ERROR" | "RENDER_FAILED" => CoreErrorCode::IoError,
        _ => CoreErrorCode::ValidationError,
    };
    storycut_core::CoreError::new(code, error.message)
}

fn shortcut_spec(tool: &str) -> Option<(&'static str, &'static [&'static str])> {
    match tool {
        "storycut_track_add" => Some(("track.add", &["track"])),
        "storycut_track_update" => Some(("track.update", &["track_id", "changes"])),
        "storycut_clip_add" => Some(("clip.add", &["clip"])),
        "storycut_clip_move" => Some((
            "clip.move",
            &["clip_id", "track_id", "start_tick", "respect_links"],
        )),
        "storycut_clip_trim" => Some((
            "clip.trim",
            &[
                "clip_id",
                "start_tick",
                "source_in_tick",
                "duration_ticks",
                "keyframe_policy",
                "respect_links",
            ],
        )),
        "storycut_clip_split" => Some(("clip.split", &["clip_id", "at_tick", "respect_links"])),
        "storycut_clip_remove" => Some((
            "clip.remove",
            &["clip_id", "respect_links", "mode", "ripple_track_ids"],
        )),
        "storycut_motion_set" => Some(("motion.set", &["clip_id", "motion"])),
        "storycut_audio_set" => Some(("audio.set", &["clip_id", "audio"])),
        "storycut_transition_set" => Some(("transition.set", &["transition"])),
        "storycut_link_create" => Some(("link.create", &["link"])),
        _ => None,
    }
}

fn project_duration(project: &storycut_core::Project) -> Result<u64, CommandError> {
    Ok(project.duration_ticks())
}

fn core_error(error: storycut_core::CoreError) -> CommandError {
    let code = match error.code() {
        "VALIDATION_ERROR" => "INVALID_ARGUMENT",
        "CONFLICT" => "DEPENDENCY_CONFLICT",
        "IO_ERROR" | "SERIALIZATION_ERROR" => "INTERNAL_ERROR",
        other => other,
    };
    CommandError::new(code, error.to_string())
}

fn internal_error(error: serde_json::Error) -> CommandError {
    CommandError::new("INTERNAL_ERROR", error.to_string())
}

fn render_error(error: storycut_render::RenderError) -> CommandError {
    let code = match error.code() {
        "UNSUPPORTED_FEATURE" => "UNSUPPORTED_FEATURE",
        "MEDIA_CHANGED" => "MEDIA_CHANGED",
        "OUTPUT_EXISTS" => "OVERWRITE_DENIED",
        "INVALID_PROJECT" | "INVALID_OPTIONS" => "INVALID_ARGUMENT",
        "PROCESS_FAILED" | "VERIFICATION_FAILED" => "RENDER_FAILED",
        _ => "INTERNAL_ERROR",
    };
    CommandError::new(code, error.to_string())
}

fn subtitle_error(error: storycut_subtitle::SubtitleError) -> CommandError {
    let code = match error.code() {
        "IO_ERROR" => "MEDIA_OFFLINE",
        "UNSUPPORTED_FEATURE" => "UNSUPPORTED_FEATURE",
        _ => "INVALID_ARGUMENT",
    };
    CommandError::new(code, error.to_string())
}

fn stable_job_id(project_id: &str, key: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(project_id.as_bytes());
    hash.update(b"\0");
    hash.update(key.as_bytes());
    format!("render-{}", &format!("{:x}", hash.finalize())[..20])
}

fn stable_preview_id(project_id: &str, key: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(project_id.as_bytes());
    hash.update(b"\0preview\0");
    hash.update(key.as_bytes());
    format!("preview-{}", &format!("{:x}", hash.finalize())[..20])
}

fn stable_preview_range_id(project_id: &str, key: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(project_id.as_bytes());
    hash.update(b"\0preview-range\0");
    hash.update(key.as_bytes());
    format!("preview-range-{}", &format!("{:x}", hash.finalize())[..20])
}

fn stable_export_id(project_id: &str, key: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(project_id.as_bytes());
    hash.update(b"\0subtitle-export\0");
    hash.update(key.as_bytes());
    format!("export-{}", &format!("{:x}", hash.finalize())[..20])
}

fn hex_sha256(bytes: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update(bytes);
    format!("{:x}", hash.finalize())
}

fn write_new_output(path: &Path, bytes: &[u8]) -> Result<(), CommandError> {
    atomic_write_new(path, bytes, "OVERWRITE_DENIED")
}

/// Persist a complete record before making its final name visible. A crash
/// during serialization or flush leaves only an unreferenced temporary file,
/// never a malformed initial/ready/result record that blocks replay.
fn write_json_new(path: &Path, value: &Value) -> Result<(), CommandError> {
    let bytes = serde_json::to_vec(value).map_err(internal_error)?;
    atomic_write_new(path, &bytes, "INTERNAL_ERROR")
}

fn atomic_write_new(
    path: &Path,
    bytes: &[u8],
    collision_code: &'static str,
) -> Result<(), CommandError> {
    let parent = path
        .parent()
        .ok_or_else(|| CommandError::new("INVALID_ARGUMENT", "Output has no parent directory"))?;
    let temporary = parent.join(format!(".storycut-{}.partial", Uuid::new_v4()));
    let _temporary_guard = TemporaryFile(temporary.clone());
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|e| CommandError::new("INTERNAL_ERROR", e.to_string()))?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| CommandError::new("INTERNAL_ERROR", e.to_string()))?;
    drop(file);
    fs::hard_link(&temporary, path).map_err(|error| {
        CommandError::new(
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                collision_code
            } else {
                "INTERNAL_ERROR"
            },
            error.to_string(),
        )
    })
}

struct TemporaryFile(PathBuf);

impl Drop for TemporaryFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// The export intent includes its bytes so a retry can finish after a crash.
fn replay_export(
    record_path: &Path,
    args: &Value,
    output: &Path,
    project_id: &str,
    revision: u64,
) -> Result<Option<Value>, CommandError> {
    if !record_path.exists() {
        return Ok(None);
    }
    let record = read_job_file(record_path)?;
    if record["request"] != *args {
        return Err(CommandError::new(
            "IDEMPOTENCY_CONFLICT",
            "Export key was used with different arguments",
        ));
    }
    let content = record["content"]
        .as_str()
        .ok_or_else(|| CommandError::new("INTERNAL_ERROR", "Export record has no content"))?;
    let expected = record["data"]["artifact"]["sha256"]
        .as_str()
        .ok_or_else(|| CommandError::new("INTERNAL_ERROR", "Export record has no digest"))?;
    if output.exists() {
        let bytes =
            fs::read(output).map_err(|e| CommandError::new("INTERNAL_ERROR", e.to_string()))?;
        if hex_sha256(&bytes) != expected {
            return Err(CommandError::new(
                "MEDIA_CHANGED",
                "Export file changed after the first attempt",
            ));
        }
    } else {
        write_new_output(output, content.as_bytes())?;
    }
    Ok(Some(success(
        Some(project_id),
        Some(revision),
        record["data"].clone(),
    )))
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 80
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'.' | b'-' | b'_'))
}

fn job_dir(workspace: &Path) -> Result<PathBuf, CommandError> {
    private_dir(workspace, "jobs", false)
}

/// Verify both private directory levels after resolving Windows junctions and symlinks.
fn private_dir(workspace: &Path, leaf: &str, create: bool) -> Result<PathBuf, CommandError> {
    let root =
        fs::canonicalize(workspace).map_err(|e| CommandError::new("PATH_DENIED", e.to_string()))?;
    let mut current = root.clone();
    for component in [".storycut", leaf] {
        let next = current.join(component);
        if create {
            match fs::create_dir(&next) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(CommandError::new(
                        "PATH_DENIED",
                        format!("Cannot create private directory: {error}"),
                    ));
                }
            }
        }
        if next.exists() {
            current = fs::canonicalize(&next)
                .map_err(|e| CommandError::new("PATH_DENIED", e.to_string()))?;
            if !current.starts_with(&root) || !current.is_dir() {
                return Err(CommandError::new(
                    "PATH_DENIED",
                    "Private directory escaped the authorized workspace",
                ));
            }
        } else {
            // The read path is allowed to remain absent; its caller reports NOT_FOUND.
            current = next;
        }
    }
    Ok(current)
}

fn authorize_project_media(
    workspace: &Path,
    project_path: &Path,
    project: &storycut_core::Project,
) -> Result<(), CommandError> {
    let authorized_project = checked_path(workspace, &project_path.to_string_lossy(), true)?;
    let base = authorized_project
        .parent()
        .ok_or_else(|| CommandError::new("PATH_DENIED", "Project has no parent directory"))?;
    for asset in &project.assets {
        let candidate = if Path::new(&asset.path).is_absolute() {
            PathBuf::from(&asset.path)
        } else {
            base.join(&asset.path)
        };
        if fs::symlink_metadata(&candidate).is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            checked_path(workspace, &candidate.to_string_lossy(), true)?;
        } else {
            checked_path(workspace, &candidate.to_string_lossy(), candidate.exists())?;
        }
    }
    Ok(())
}

fn read_job_file(path: &Path) -> Result<Value, CommandError> {
    if fs::symlink_metadata(path).is_ok_and(|metadata| !metadata.file_type().is_file()) {
        return Err(CommandError::new(
            "PATH_DENIED",
            "Private record is not a regular file",
        ));
    }
    let content = fs::read(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            CommandError::new("NOT_FOUND", "Job not found")
        } else {
            CommandError::new("INTERNAL_ERROR", e.to_string())
        }
    })?;
    serde_json::from_slice(&content).map_err(internal_error)
}

fn lock_job(jobs: &Path, job_id: &str, wait: bool) -> Result<Option<File>, CommandError> {
    let path = jobs.join(format!("{job_id}.lock"));
    let file = match OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let metadata = fs::symlink_metadata(&path)
                .map_err(|e| CommandError::new("PATH_DENIED", e.to_string()))?;
            if !metadata.file_type().is_file() {
                return Err(CommandError::new(
                    "PATH_DENIED",
                    "Job lock is not a regular file",
                ));
            }
            OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .map_err(|e| CommandError::new("PATH_DENIED", e.to_string()))?
        }
        Err(error) => return Err(CommandError::new("INTERNAL_ERROR", error.to_string())),
    };
    if wait {
        file.lock_exclusive()
            .map_err(|e| CommandError::new("INTERNAL_ERROR", e.to_string()))?;
        Ok(Some(file))
    } else {
        match file.try_lock_exclusive() {
            Ok(()) => Ok(Some(file)),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
            Err(error) => Err(CommandError::new("INTERNAL_ERROR", error.to_string())),
        }
    }
}

/// With the job lock held, a missing result means the former worker stopped.
fn completed_or_interrupted_job(
    workspace: &Path,
    jobs: &Path,
    job_id: &str,
) -> Result<Value, CommandError> {
    let result = jobs.join(format!("{job_id}.result.json"));
    if result.exists() {
        return read_job_file(&result);
    }
    let ready = jobs.join(format!("{job_id}.ready.json"));
    if ready.exists() {
        return finalize_ready_job(workspace, jobs, job_id);
    }
    let initial = jobs.join(format!("{job_id}.initial.json"));
    let mut record = read_job_file(&initial)?;
    let mut job = record["job"].clone();
    if !job.is_object() {
        return Err(CommandError::new(
            "INTERNAL_ERROR",
            "Job record is malformed",
        ));
    }
    job["state"] = json!("interrupted");
    job["failure_code"] = json!("JOB_INTERRUPTED");
    job["last_event_seq"] = json!(2);
    job["progress"] = Value::Null;
    let completed = json!({"job":job});
    write_json_new(&result, &completed)?;
    record["job"] = job;
    Ok(record)
}

fn existing_job_response(
    workspace: &Path,
    jobs: &Path,
    job_id: &str,
    args: &Value,
    project_id: &str,
    revision: u64,
) -> Result<Option<Value>, CommandError> {
    let initial = jobs.join(format!("{job_id}.initial.json"));
    if !initial.exists() {
        return Ok(None);
    }
    let old = read_job_file(&initial)?;
    if old["request"] != *args {
        return Err(CommandError::new(
            "IDEMPOTENCY_CONFLICT",
            "Job key was already used with different arguments",
        ));
    }
    let record = completed_or_interrupted_job(workspace, jobs, job_id)?;
    Ok(Some(success(
        Some(project_id),
        Some(revision),
        json!({"job":record["job"]}),
    )))
}

fn write_ready_job(jobs: &Path, job_id: &str, job: &Value) -> Result<(), CommandError> {
    let path = jobs.join(format!("{job_id}.ready.json"));
    write_json_new(&path, &json!({"job":job}))
}

fn sha256_file(path: &Path) -> Result<(String, u64), CommandError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|e| CommandError::new("MEDIA_OFFLINE", e.to_string()))?;
    if !metadata.file_type().is_file() {
        return Err(CommandError::new(
            "PATH_DENIED",
            "Job artifact is not a regular file",
        ));
    }
    let mut file =
        File::open(path).map_err(|e| CommandError::new("MEDIA_OFFLINE", e.to_string()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|e| CommandError::new("MEDIA_OFFLINE", e.to_string()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok((format!("{:x}", hasher.finalize()), metadata.len()))
}

/// Finish a staged artifact whose digest was persisted before publication.
fn finalize_ready_job(workspace: &Path, jobs: &Path, job_id: &str) -> Result<Value, CommandError> {
    let initial = read_job_file(&jobs.join(format!("{job_id}.initial.json")))?;
    let ready = read_job_file(&jobs.join(format!("{job_id}.ready.json")))?;
    let job = ready["job"].clone();
    if job["state"] != "succeeded" {
        return Err(CommandError::new(
            "INTERNAL_ERROR",
            "Ready job is not succeeded",
        ));
    }
    let artifact = job["artifacts"]
        .as_array()
        .and_then(|items| items.first())
        .ok_or_else(|| CommandError::new("INTERNAL_ERROR", "Ready job has no artifact"))?;
    let expected_sha = artifact["sha256"]
        .as_str()
        .ok_or_else(|| CommandError::new("INTERNAL_ERROR", "Ready artifact has no digest"))?;
    let expected_size = artifact["size_bytes"]
        .as_u64()
        .ok_or_else(|| CommandError::new("INTERNAL_ERROR", "Ready artifact has no size"))?;
    let kind = job["kind"]
        .as_str()
        .ok_or_else(|| CommandError::new("INTERNAL_ERROR", "Ready job has no kind"))?;
    let (stage, output) = match kind {
        "render" => (
            jobs.join(format!("{job_id}.stage.mp4")),
            checked_path(workspace, field_str(&initial["request"], "path")?, false)?,
        ),
        "preview_frame" => (
            jobs.join(format!("{job_id}.stage.png")),
            private_dir(workspace, "previews", false)?.join(format!("{job_id}.png")),
        ),
        "preview_range" => (
            jobs.join(format!("{job_id}.stage.mp4")),
            private_dir(workspace, "previews", false)?.join(format!("{job_id}.mp4")),
        ),
        _ => {
            return Err(CommandError::new(
                "INTERNAL_ERROR",
                "Unknown ready job kind",
            ));
        }
    };
    let root =
        fs::canonicalize(workspace).map_err(|e| CommandError::new("PATH_DENIED", e.to_string()))?;
    let relative = output
        .strip_prefix(&root)
        .map_err(|_| CommandError::new("PATH_DENIED", "Ready output escaped workspace"))?;
    if artifact["relative_path"].as_str() != Some(relative.to_string_lossy().as_ref()) {
        return Err(CommandError::new(
            "INTERNAL_ERROR",
            "Ready artifact path mismatch",
        ));
    }
    if output.exists() {
        let (digest, size) = sha256_file(&output)?;
        if digest != expected_sha || size != expected_size {
            return Err(CommandError::new(
                "MEDIA_CHANGED",
                "Published job artifact changed",
            ));
        }
    } else {
        let (digest, size) = sha256_file(&stage)?;
        if digest != expected_sha || size != expected_size {
            return Err(CommandError::new(
                "MEDIA_CHANGED",
                "Staged job artifact changed",
            ));
        }
        match fs::hard_link(&stage, &output) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let (digest, size) = sha256_file(&output)?;
                if digest != expected_sha || size != expected_size {
                    return Err(CommandError::new(
                        "MEDIA_CHANGED",
                        "Published job artifact changed",
                    ));
                }
            }
            Err(error) => return Err(CommandError::new("INTERNAL_ERROR", error.to_string())),
        }
    }
    let result = jobs.join(format!("{job_id}.result.json"));
    let record = json!({"job":job});
    if !result.exists() {
        write_json_new(&result, &record)?;
    }
    let _ = fs::remove_file(&stage);
    Ok(record)
}

fn command_available(cmd: &str, flag: &str) -> bool {
    Command::new(cmd)
        .arg(flag)
        .output()
        .is_ok_and(|v| v.status.success())
}

fn require_object_keys(value: &Value, allowed: &[&str]) -> Result<(), CommandError> {
    let object = value
        .as_object()
        .ok_or_else(|| CommandError::new("INVALID_ARGUMENT", "Arguments must be an object"))?;
    if let Some(key) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(CommandError::new(
            "INVALID_ARGUMENT",
            format!("Unknown field: {key}"),
        ));
    }
    Ok(())
}

fn field_str<'a>(value: &'a Value, key: &str) -> Result<&'a str, CommandError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| CommandError::new("INVALID_ARGUMENT", format!("Missing or invalid {key}")))
}

fn field_u64(value: &Value, key: &str) -> Result<u64, CommandError> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| CommandError::new("INVALID_ARGUMENT", format!("Missing or invalid {key}")))
}

fn field_bool(value: &Value, key: &str) -> Result<bool, CommandError> {
    value
        .get(key)
        .and_then(Value::as_bool)
        .ok_or_else(|| CommandError::new("INVALID_ARGUMENT", format!("Missing or invalid {key}")))
}

fn field_array<'a>(value: &'a Value, key: &str) -> Result<&'a [Value], CommandError> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| CommandError::new("INVALID_ARGUMENT", format!("Missing or invalid {key}")))
}

/// Resolve an existing input or an output in an existing directory without
/// allowing a junction or `..` to escape the explicitly chosen workspace.
fn checked_path(workspace: &Path, raw: &str, existing: bool) -> Result<PathBuf, CommandError> {
    let root = fs::canonicalize(workspace).map_err(|e| {
        CommandError::new("PATH_DENIED", format!("Workspace is not accessible: {e}"))
    })?;
    let given = Path::new(raw);
    let path = if given.is_absolute() {
        given.to_path_buf()
    } else {
        root.join(given)
    };
    let resolved = if existing {
        fs::canonicalize(&path)
            .map_err(|e| CommandError::new("NOT_FOUND", format!("Path is not accessible: {e}")))?
    } else {
        let name = path
            .file_name()
            .ok_or_else(|| CommandError::new("INVALID_ARGUMENT", "Invalid output filename"))?;
        let parent = path
            .parent()
            .ok_or_else(|| CommandError::new("INVALID_ARGUMENT", "Invalid output directory"))?;
        let real_parent = fs::canonicalize(parent).map_err(|e| {
            CommandError::new(
                "PATH_DENIED",
                format!("Output directory is not accessible: {e}"),
            )
        })?;
        real_parent.join(name)
    };
    if !resolved.starts_with(&root) {
        return Err(CommandError::new(
            "PATH_DENIED",
            "Path is outside the authorized workspace",
        ));
    }
    Ok(resolved)
}

fn success(project_id: Option<&str>, revision: Option<u64>, data: Value) -> Value {
    json!({
        "api_version": "0.2.0",
        "ok": true,
        "request_id": Uuid::new_v4().to_string(),
        "project_id": project_id,
        "revision": revision,
        "data": data,
        "error": null,
        "warnings": []
    })
}

fn with_projection_warning(mut response: Value, warning: Option<&str>) -> Value {
    if let Some(message) = warning {
        response["warnings"] = json!([{
            "code": "PROJECT_EXPORT_DEFERRED",
            "message": message
        }]);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use storycut_core::{ProbeStatus, Rational, Stream};

    fn fixture_image(id: String) -> Asset {
        Asset {
            id,
            kind: AssetKind::Image,
            path: "fixture.png".into(),
            probe_status: ProbeStatus::Probed,
            duration_ticks: None,
            sha256: None,
            streams: Vec::new(),
        }
    }

    fn fixture_audio(id: &str, duration_seconds: u64) -> Asset {
        Asset {
            id: id.into(),
            kind: AssetKind::Audio,
            path: "fixture.wav".into(),
            probe_status: ProbeStatus::Probed,
            duration_ticks: Some(duration_seconds * TIMEBASE),
            sha256: None,
            streams: vec![Stream {
                index: 0,
                kind: StreamKind::Audio,
                time_base: Rational {
                    num: 1,
                    den: 48_000,
                },
                sample_rate: Some(48_000),
            }],
        }
    }

    fn fixture_store(workspace: &Path, assets: Vec<Asset>) -> (ProjectStore, String) {
        let store = ProjectStore::create(
            workspace,
            "fixture.storycut.json",
            "fixture",
            Canvas::default(),
            48_000,
            "fixture-create",
        )
        .unwrap();
        store
            .import_assets(0, "fixture-import", false, assets)
            .unwrap();
        let project_id = store.snapshot().unwrap().project_id;
        (store, project_id)
    }

    #[test]
    fn focal_preset_wrong_type_is_rejected_before_media_access() {
        let project = Project::empty("preset-test", "preset", Canvas::default(), 48_000);
        for invalid in [json!(1), json!(false), Value::Null] {
            let error = plan_focal_motion(
                Path::new("."),
                &project,
                &json!({"targets":[{"clip_id":"missing"}],"preset":invalid}),
            )
            .unwrap_err();
            assert!(error.to_string().contains("preset must be a string"));
        }
    }

    #[test]
    fn storyboard_defaults_seventy_images_and_music_loop_end_with_exact_tail_fade_and_replay() {
        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path();
        let images = (0..70)
            .map(|index| fixture_image(format!("image-{index:03}")))
            .collect::<Vec<_>>();
        let mut assets = images;
        assets.push(fixture_audio("music", 3));
        let (store, project_id) = fixture_store(workspace, assets);
        let project = store.snapshot().unwrap();
        let request = json!({
            "project_id":project_id,
            "expected_revision":project.revision,
            "idempotency_key":"story-assembly-key",
            "dry_run":true,
            "items":(0..70).map(|index| json!({"asset_id":format!("image-{index:03}")})).collect::<Vec<_>>(),
            "audio_tracks":[{
                "role":"music",
                "clips":[{
                    "asset_id":"music","start_seconds":0,"loop_to_visual_end":true,
                    "fade_out_seconds":1
                }]
            }]
        });

        let preview = dispatch(workspace, "storycut_storyboard_assemble", request.clone()).unwrap();
        assert_eq!(preview["data"]["committed"], false);
        assert_eq!(
            preview["data"]["visual_duration_ticks"],
            json!(700 * TIMEBASE)
        );
        assert_eq!(
            preview["data"]["audio_duration_ticks"],
            json!(700 * TIMEBASE)
        );
        assert_eq!(
            store.snapshot().unwrap().revision,
            1,
            "dry run must not write"
        );

        let mut commit = request.clone();
        commit["dry_run"] = json!(false);
        let committed =
            dispatch(workspace, "storycut_storyboard_assemble", commit.clone()).unwrap();
        assert_eq!(committed["data"]["committed"], true);
        let project = store.snapshot().unwrap();
        assert_eq!(project.revision, 2);
        assert_eq!(
            project.clips.iter().filter(|clip| clip.is_visual()).count(),
            70
        );
        let music_clips = project
            .clips
            .iter()
            .filter_map(|clip| match clip {
                Clip::Audio(audio) if audio.asset_id == "music" => Some(audio),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(music_clips.len(), 234);
        let last = music_clips
            .iter()
            .max_by_key(|clip| clip.start_tick)
            .unwrap();
        assert_eq!(last.start_tick, 699 * TIMEBASE);
        assert_eq!(last.duration_ticks, TIMEBASE);
        assert_eq!(last.audio.domain_duration_ticks, TIMEBASE);
        assert_eq!(last.audio.sample_offset_tick, 0);
        assert_eq!(
            last.audio.fade_out_ticks, TIMEBASE,
            "a fade equal to the 1s tail is valid"
        );

        let stale = dispatch(
            workspace,
            "storycut_storyboard_assemble",
            json!({
                "project_id":project_id,"expected_revision":1,"idempotency_key":"stale-story",
                "dry_run":false,"items":[{"asset_id":"image-000"}]
            }),
        )
        .unwrap_err();
        assert_eq!(stale.code, "REVISION_CONFLICT");
        assert_eq!(store.snapshot().unwrap().revision, 2);

        let first_visual_id = project
            .clips
            .iter()
            .find(|clip| clip.is_visual())
            .unwrap()
            .id()
            .to_owned();
        let invalid_focus = dispatch(
            workspace,
            "storycut_focal_motion_apply",
            json!({
                "project_id":project_id,"expected_revision":2,"idempotency_key":"bad-focus",
                "dry_run":false,"targets":[{"clip_id":first_visual_id}],"from_scale":0.9
            }),
        )
        .unwrap_err();
        assert_eq!(invalid_focus.code, "INVALID_ARGUMENT");
        assert_eq!(
            store.snapshot().unwrap().revision,
            2,
            "invalid scale must not write or panic"
        );

        let later_edit = store
            .apply(
                2,
                "later-edit",
                false,
                vec![Operation::TrackAdd {
                    track: Track {
                        id: "later-track".into(),
                        name: "later edit".into(),
                        kind: TrackKind::Audio,
                        locked: false,
                        enabled: true,
                        muted: false,
                        solo: false,
                        gain_db: 0.0,
                        ducking: None,
                    },
                }],
            )
            .unwrap();
        assert_eq!(later_edit.revision, 3);
        store.undo(3, "undo-later-edit", false).unwrap();
        assert_eq!(store.snapshot().unwrap().revision, 4);

        let replay = dispatch(workspace, "storycut_storyboard_assemble", commit).unwrap();
        assert_eq!(
            replay["revision"], 2,
            "replay returns the original committed revision"
        );
        assert_eq!(
            replay["data"]["created_clip_ids"],
            committed["data"]["created_clip_ids"]
        );
        assert_eq!(
            store.snapshot().unwrap().revision,
            4,
            "replay after edit+undo must not reapply clips"
        );
        assert_eq!(store.snapshot().unwrap().clips.len(), 304);
    }

    #[test]
    fn storyboard_rejects_locked_visual_track_and_wrong_typed_options_without_writes() {
        let temporary = tempfile::tempdir().unwrap();
        let (store, project_id) = fixture_store(
            temporary.path(),
            vec![fixture_image("image".into()), fixture_audio("music", 3)],
        );
        store
            .apply(
                1,
                "add-locked-track",
                false,
                vec![Operation::TrackAdd {
                    track: Track {
                        id: "locked-video".into(),
                        name: "locked".into(),
                        kind: TrackKind::Video,
                        locked: true,
                        enabled: true,
                        muted: false,
                        solo: false,
                        gain_db: 0.0,
                        ducking: None,
                    },
                }],
            )
            .unwrap();
        let locked = dispatch(
            temporary.path(),
            "storycut_storyboard_assemble",
            json!({
                "project_id":project_id,"expected_revision":2,"idempotency_key":"locked-story",
                "dry_run":false,"visual_track_id":"locked-video","items":[{"asset_id":"image"}]
            }),
        )
        .unwrap_err();
        assert_eq!(locked.code, "INVALID_ARGUMENT");
        assert_eq!(store.snapshot().unwrap().revision, 2);

        for (key, options) in [
            ("wrong-audio-tracks", json!({"audio_tracks":"music"})),
            (
                "wrong-audio-gain",
                json!({"audio_tracks":[{"role":"music","clips":[{"asset_id":"music","start_seconds":0,"gain_db":"loud"}]}]}),
            ),
            (
                "wrong-loop-type",
                json!({"audio_tracks":[{"role":"music","clips":[{"asset_id":"music","start_seconds":0,"loop_to_visual_end":"yes"}]}]}),
            ),
            (
                "fade-exceeds-loop-tail",
                json!({"audio_tracks":[{"role":"music","clips":[{"asset_id":"music","start_seconds":0,"loop_to_visual_end":true,"fade_out_seconds":1.1}]}]}),
            ),
        ] {
            let mut request = json!({
                "project_id":project_id,"expected_revision":2,"idempotency_key":key,
                "dry_run":false,"items":[{"asset_id":"image"}]
            });
            for (name, value) in options.as_object().unwrap() {
                request[name] = value.clone();
            }
            let error =
                dispatch(temporary.path(), "storycut_storyboard_assemble", request).unwrap_err();
            assert_eq!(error.code, "INVALID_ARGUMENT");
            assert_eq!(
                store.snapshot().unwrap().revision,
                2,
                "invalid {key} wrote project state"
            );
        }
    }

    #[test]
    fn storyboard_crossfade_reports_frame_grid_rounding_and_audio_ids_do_not_collide() {
        let assets = vec![
            fixture_image("still-a".into()),
            fixture_image("still-b".into()),
            fixture_audio("music", 3),
        ];
        let mut project = Project::empty("planner-test", "planner", Canvas::default(), 48_000);
        project.assets = assets;
        let mut request = json!({
            "items":[{"asset_id":"still-a"},{"asset_id":"still-b"}],
            "transition":{"kind":"cross_dissolve","duration_seconds":0.02}
        });
        let (_, transition_data) =
            plan_storyboard(Path::new("."), &project, &request, "transition-key").unwrap();
        assert_eq!(
            transition_data["warnings"][0]["code"],
            "TRANSITION_ROUNDED_TO_FRAME"
        );
        assert_eq!(
            transition_data["warnings"][0]["duration_ticks"],
            json!(TIMEBASE / 30)
        );

        let clips = (0..101)
            .map(|_| json!({"asset_id":"music","start_seconds":0}))
            .collect::<Vec<_>>();
        request = json!({
            "items":[{"asset_id":"still-a"}],
            "audio_tracks":[
                {"role":"music","clips":clips},
                {"role":"dialogue","clips":[{"asset_id":"music","start_seconds":0}]}
            ]
        });
        let (operations, _) =
            plan_storyboard(Path::new("."), &project, &request, "collision-key").unwrap();
        let mut ids = std::collections::HashSet::new();
        for operation in operations {
            if let Operation::ClipAdd { clip } = operation {
                assert!(
                    ids.insert(clip.id().to_owned()),
                    "duplicate generated clip ID {}",
                    clip.id()
                );
            }
        }
    }

    #[test]
    fn music_loop_never_rounds_past_visual_end_on_a_different_audio_sample_grid() {
        let mut canvas = Canvas::default();
        canvas.fps = Rational { num: 24, den: 1 };
        let mut project = Project::empty("sample-grid-test", "sample grid", canvas, 44_100);
        let mut audio = fixture_audio("music", 3);
        audio.streams[0].sample_rate = Some(44_100);
        project.assets = vec![fixture_image("still".into()), audio];
        let (operations, summary) = plan_storyboard(
            Path::new("."),
            &project,
            &json!({
                "items":[{"asset_id":"still","duration_seconds":0.125}],
                "audio_tracks":[{"role":"music","clips":[{
                    "asset_id":"music","start_seconds":0,"loop_to_visual_end":true
                }]}]
            }),
            "different-grid-key",
        )
        .unwrap();
        let visual_end = summary["visual_duration_ticks"].as_u64().unwrap();
        let audio_clip = operations
            .iter()
            .find_map(|operation| match operation {
                Operation::ClipAdd {
                    clip: Clip::Audio(audio),
                } => Some(audio),
                _ => None,
            })
            .unwrap();
        let audio_end = audio_clip.start_tick + audio_clip.duration_ticks;
        let sample_ticks = TIMEBASE / 44_100;
        assert!(
            audio_end <= visual_end,
            "music loop must not extend the video"
        );
        assert!(
            visual_end - audio_end < sample_ticks,
            "only a sub-sample remainder may be silent"
        );
    }

    #[test]
    fn storyboard_summary_lists_automatically_created_original_audio_clip() {
        let mut project = Project::empty(
            "linked-audio-test",
            "linked audio",
            Canvas::default(),
            48_000,
        );
        project.assets = vec![Asset {
            id: "video".into(),
            kind: AssetKind::Video,
            path: "fixture.mp4".into(),
            probe_status: ProbeStatus::Probed,
            duration_ticks: Some(5 * TIMEBASE),
            sha256: None,
            streams: vec![
                Stream {
                    index: 0,
                    kind: StreamKind::Video,
                    time_base: Rational {
                        num: 1,
                        den: 90_000,
                    },
                    sample_rate: None,
                },
                Stream {
                    index: 1,
                    kind: StreamKind::Audio,
                    time_base: Rational {
                        num: 1,
                        den: 48_000,
                    },
                    sample_rate: Some(48_000),
                },
            ],
        }];
        let (operations, summary) = plan_storyboard(
            Path::new("."),
            &project,
            &json!({"items":[{"asset_id":"video"}]}),
            "linked-audio-key",
        )
        .unwrap();
        let operation_clip_ids = operations
            .iter()
            .filter_map(|operation| match operation {
                Operation::ClipAdd { clip } => Some(clip.id().to_owned()),
                _ => None,
            })
            .collect::<std::collections::HashSet<_>>();
        let summary_clip_ids = summary["created_clip_ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap().to_owned())
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(operation_clip_ids, summary_clip_ids);
        assert_eq!(
            operations
                .iter()
                .filter(|operation| matches!(operation, Operation::LinkCreate { .. }))
                .count(),
            1
        );
    }

    #[test]
    fn supported_tools_advertise_range_preview_only_after_dispatch_exists() {
        let tools = supported_tools();
        assert!(tools.iter().any(|tool| tool == "storycut_preview_range"));
        assert_eq!(
            tools
                .iter()
                .filter(|tool| *tool == "storycut_preview_range")
                .count(),
            1
        );
    }

    #[test]
    fn preview_range_rejects_odd_width_before_loading_project() {
        let temporary = tempfile::tempdir().unwrap();
        let error = dispatch(
            temporary.path(),
            "storycut_preview_range",
            json!({
                "project_id":"p-test",
                "revision":0,
                "idempotency_key":"odd-width",
                "width":321,
                "include_subtitles":false,
                "start_tick":0,
                "duration_ticks":705600000
            }),
        )
        .unwrap_err();

        assert_eq!(error.code, "INVALID_ARGUMENT");
        assert!(error.message.contains("even"));
    }

    #[test]
    fn ready_job_recovers_after_publish_or_before_publish() {
        for final_already_published in [false, true] {
            let temporary = tempfile::tempdir().unwrap();
            let workspace = temporary.path();
            let jobs = private_dir(workspace, "jobs", true).unwrap();
            let job_id = "render-recovery-test";
            let output = workspace.join("finished.mp4");
            let stage = jobs.join(format!("{job_id}.stage.mp4"));
            let bytes = b"verified staged artifact";
            let digest = hex_sha256(bytes);
            fs::write(
                jobs.join(format!("{job_id}.initial.json")),
                serde_json::to_vec(&json!({
                    "request":{"path":output.to_string_lossy()},
                    "job":{"kind":"render","state":"running"}
                }))
                .unwrap(),
            )
            .unwrap();
            let ready_job = json!({
                "job_id":job_id,"project_id":"project","source_revision":1,
                "kind":"render","state":"succeeded","progress":1.0,
                "last_event_seq":2,"failure_code":null,
                "artifacts":[{"artifact_id":"artifact-test","relative_path":"finished.mp4",
                    "mime_type":"video/mp4","sha256":digest,"size_bytes":bytes.len()}]
            });
            write_ready_job(&jobs, job_id, &ready_job).unwrap();
            if final_already_published {
                fs::write(&output, bytes).unwrap();
            } else {
                fs::write(&stage, bytes).unwrap();
            }
            let recovered = completed_or_interrupted_job(workspace, &jobs, job_id).unwrap();
            assert_eq!(recovered["job"]["state"], "succeeded");
            assert_eq!(fs::read(&output).unwrap(), bytes);
            assert!(jobs.join(format!("{job_id}.result.json")).is_file());
            assert!(!stage.exists());
        }
    }

    #[test]
    fn ready_preview_range_recovers_to_private_mp4_path() {
        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path();
        let jobs = private_dir(workspace, "jobs", true).unwrap();
        let previews = private_dir(workspace, "previews", true).unwrap();
        let root = fs::canonicalize(workspace).unwrap();
        let job_id = "preview-range-recovery-test";
        let stage = jobs.join(format!("{job_id}.stage.mp4"));
        let bytes = b"verified preview video";
        let digest = hex_sha256(bytes);
        fs::write(
            jobs.join(format!("{job_id}.initial.json")),
            serde_json::to_vec(&json!({
                "request":{"project_id":"project","revision":1,"idempotency_key":"key",
                    "width":320,"include_subtitles":false,"start_tick":0,"duration_ticks":705600000},
                "job":{"kind":"preview_range","state":"running"}
            }))
            .unwrap(),
        )
        .unwrap();
        let ready_job = json!({
            "job_id":job_id,"project_id":"project","source_revision":1,
            "kind":"preview_range","state":"succeeded","progress":1.0,
            "last_event_seq":2,"failure_code":null,
            "artifacts":[{"artifact_id":"artifact-test","relative_path":
                previews.join(format!("{job_id}.mp4")).strip_prefix(&root).unwrap().to_string_lossy(),"mime_type":"video/mp4",
                "sha256":digest,"size_bytes":bytes.len()}]
        });
        write_ready_job(&jobs, job_id, &ready_job).unwrap();
        fs::write(&stage, bytes).unwrap();

        let recovered = completed_or_interrupted_job(workspace, &jobs, job_id).unwrap();
        let output = previews.join(format!("{job_id}.mp4"));
        assert_eq!(recovered["job"]["kind"], "preview_range");
        assert_eq!(recovered["job"]["state"], "succeeded");
        assert_eq!(fs::read(&output).unwrap(), bytes);
        assert!(jobs.join(format!("{job_id}.result.json")).is_file());
        assert!(!stage.exists());
    }

    #[test]
    fn atomic_json_publish_does_not_expose_partial_or_replace_existing_record() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("job.result.json");
        let first = json!({"job":{"state":"succeeded"}});
        write_json_new(&path, &first).unwrap();
        assert_eq!(read_job_file(&path).unwrap(), first);

        let error = write_json_new(&path, &json!({"job":{"state":"failed"}})).unwrap_err();
        assert_eq!(error.code, "INTERNAL_ERROR");
        assert_eq!(read_job_file(&path).unwrap(), first);
        assert_eq!(
            fs::read_dir(temporary.path())
                .unwrap()
                .filter_map(Result::ok)
                .count(),
            1,
            "atomic publication should clean its temporary file"
        );
    }
}

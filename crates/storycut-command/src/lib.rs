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
use storycut_core::{Asset, Canvas, Operation, ProjectStore, SubtitleFormat};
use uuid::Uuid;

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
            let options = json!({
                "range_start_tick":start_tick,
                "range_end_tick":end_tick,
                "subtitle_mode":if include_subtitles {"burn"} else {"none"},
                "encoder":"h264_cpu",
                "overwrite":false
            });
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

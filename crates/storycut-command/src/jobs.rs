//! Background export workers owned by the process serving the workspace.
use super::*;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use storycut_render::RenderControl;

type Key = (PathBuf, String);
type Registry = HashMap<Key, Arc<Active>>;
static ACTIVE: OnceLock<Mutex<Registry>> = OnceLock::new();

struct Active {
    job: Mutex<Value>,
    request: Value,
    control: RenderControl,
}

fn registry() -> &'static Mutex<Registry> {
    ACTIVE.get_or_init(Mutex::default)
}
fn key(workspace: &Path, id: &str) -> Result<Key, CommandError> {
    Ok((
        fs::canonicalize(workspace).map_err(|e| CommandError::new("PATH_DENIED", e.to_string()))?,
        id.to_owned(),
    ))
}
fn poisoned<T>(_: std::sync::PoisonError<T>) -> CommandError {
    CommandError::new("INTERNAL_ERROR", "Export worker state is unavailable")
}
fn active(workspace: &Path, id: &str) -> Result<Option<Arc<Active>>, CommandError> {
    Ok(registry()
        .lock()
        .map_err(poisoned)?
        .get(&key(workspace, id)?)
        .cloned())
}
fn response(active: &Active) -> Result<Value, CommandError> {
    let mut job = active.job.lock().map_err(poisoned)?.clone();
    if job["state"] == "running" || job["state"] == "cancelling" {
        job["progress"] = json!(active.control.progress());
    }
    Ok(success(
        job["project_id"].as_str(),
        job["source_revision"].as_u64(),
        job_data(&job),
    ))
}

pub(super) fn get(workspace: &Path, id: &str) -> Result<Option<Value>, CommandError> {
    active(workspace, id)?.map(|job| response(&job)).transpose()
}

pub(super) fn replay(
    workspace: &Path,
    id: &str,
    request: &Value,
) -> Result<Option<Value>, CommandError> {
    if let Some(job) = active(workspace, id)? {
        if job.request != *request {
            return Err(CommandError::new(
                "IDEMPOTENCY_CONFLICT",
                "Job key was already used with different arguments",
            ));
        }
        return Ok(Some(response(&job)?));
    }
    Ok(None)
}

pub(super) fn cancel(workspace: &Path, args: &Value) -> Result<Value, CommandError> {
    require_object_keys(args, &["job_id", "idempotency_key"])?;
    let id = field_str(args, "job_id")?;
    if !valid_id(id) || !valid_id(field_str(args, "idempotency_key")?) {
        return Err(CommandError::new(
            "INVALID_ARGUMENT",
            "Invalid job_id or idempotency_key",
        ));
    }
    if let Some(active) = active(workspace, id)? {
        // The same gate protects final publication: cancellation cannot race
        // a succeeded publication into a cancelled state.
        let mut job = active.job.lock().map_err(poisoned)?;
        if job["state"] == "running" || job["state"] == "queued" {
            active.control.cancel();
            job["state"] = json!("cancelling");
            job["last_event_seq"] = json!(2);
        }
        drop(job);
        return response(&active);
    }
    let response = dispatch(
        workspace,
        "storycut_job_get",
        json!({"job_id":id,"include_preview":false}),
    )?;
    if response["data"]["job"]["state"] == "running" {
        return Err(CommandError::new(
            "UNSUPPORTED_FEATURE",
            "Cancellation is supported for daemon background exports only",
        ));
    }
    Ok(response)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn start(
    workspace: &Path,
    jobs: &Path,
    id: &str,
    request: &Value,
    job: &Value,
    snapshot: Value,
    project_path: PathBuf,
    lease: Option<File>,
) -> Result<Value, CommandError> {
    let identity = key(workspace, id)?;
    let active = Arc::new(Active {
        job: Mutex::new(job.clone()),
        request: request.clone(),
        control: RenderControl::default(),
    });
    let mut registry_guard = registry().lock().map_err(poisoned)?;
    // Bound expensive jobs independently of the IPC request queue.
    if registry_guard
        .keys()
        .filter(|(root, _)| *root == identity.0)
        .count()
        >= 2
    {
        let mut failed = job.clone();
        failed["state"] = json!("failed");
        failed["failure_code"] = json!("JOB_BUSY");
        failed["last_event_seq"] = json!(2);
        write_json_new(
            &jobs.join(format!("{id}.result.json")),
            &json!({"job":failed}),
        )?;
        return Err(CommandError::new(
            "JOB_BUSY",
            "At most two exports may run in this workspace; retry with a new key after completion",
        ));
    }
    registry_guard.insert(identity.clone(), active.clone());
    drop(registry_guard);
    let worker = active.clone();
    let worker_identity = identity.clone();
    let jobs = jobs.to_owned();
    let worker_jobs = jobs.clone();
    let workspace = workspace.to_owned();
    let spawn = std::thread::Builder::new()
        .name(format!("storycut-{id}"))
        .spawn(move || {
            let _lease = lease;
            let mut staged_by_worker = false;
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                finish(
                    &workspace,
                    &worker_jobs,
                    &worker_identity.1,
                    &worker,
                    &snapshot,
                    &project_path,
                    &mut staged_by_worker,
                )
            }));
            let outcome = outcome.unwrap_or_else(|_| {
                Err(CommandError::new(
                    "INTERNAL_ERROR",
                    "Export worker stopped unexpectedly",
                ))
            });
            if let Err(error) = outcome {
                eprintln!("StoryCut export failed: {error}");
                let mut job = worker.job.lock().unwrap_or_else(|e| e.into_inner());
                job["state"] = json!(if worker.control.check().is_err() {
                    "cancelled"
                } else {
                    "failed"
                });
                job["failure_code"] = json!(if worker.control.check().is_err() {
                    "CANCELLED"
                } else {
                    &error.code
                });
                job["progress"] = Value::Null;
                job["failure_message"] = json!(error.message);
                job["last_event_seq"] = json!(3);
                job["artifacts"] = json!([]);
                let result = worker_jobs.join(format!("{}.result.json", worker_identity.1));
                if !result.exists()
                    && let Err(error) = write_json_new(&result, &json!({"job":*job}))
                {
                    eprintln!("StoryCut could not persist export failure: {error}");
                }
                if staged_by_worker {
                    let _ = fs::remove_file(
                        worker_jobs.join(format!("{}.stage.mp4", worker_identity.1)),
                    );
                }
            }
            registry()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&worker_identity);
        });
    if let Err(error) = spawn {
        registry().lock().map_err(poisoned)?.remove(&identity);
        let mut failed = job.clone();
        failed["state"] = json!("failed");
        failed["failure_code"] = json!("INTERNAL_ERROR");
        failed["last_event_seq"] = json!(2);
        write_json_new(
            &jobs.join(format!("{id}.result.json")),
            &json!({"job":failed}),
        )?;
        return Err(CommandError::new("INTERNAL_ERROR", error.to_string()));
    }
    response(&active)
}

fn finish(
    workspace: &Path,
    jobs: &Path,
    id: &str,
    active: &Active,
    snapshot: &Value,
    project_path: &Path,
    staged_by_worker: &mut bool,
) -> Result<(), CommandError> {
    let stage = jobs.join(format!("{id}.stage.mp4"));
    let report = storycut_render::render_with_control(
        snapshot,
        project_path,
        workspace,
        &stage,
        &active.request,
        &active.control,
    )
    .map_err(render_error)?;
    *staged_by_worker = true;
    // Re-check cancellation while holding the publication gate.
    let mut job = active.job.lock().map_err(poisoned)?;
    active.control.check().map_err(render_error)?;
    let output = checked_path(workspace, field_str(&active.request, "path")?, false)?;
    // A foreign file appearing during encoding is never accepted as ours.
    if output.exists() {
        return Err(CommandError::new(
            "OVERWRITE_DENIED",
            "Output appeared during export",
        ));
    }
    let root =
        fs::canonicalize(workspace).map_err(|e| CommandError::new("PATH_DENIED", e.to_string()))?;
    let relative = output
        .strip_prefix(&root)
        .map_err(|_| CommandError::new("PATH_DENIED", "Output escaped workspace"))?;
    let metadata =
        fs::metadata(&stage).map_err(|e| CommandError::new("INTERNAL_ERROR", e.to_string()))?;
    let ready = job_with_master_loudness(
        json!({
            "job_id":id,"project_id":job["project_id"],"source_revision":job["source_revision"],"kind":"render",
            "state":"succeeded","progress":1.0,"last_event_seq":3,"failure_code":null,
            "artifacts":[{"artifact_id":format!("artifact-{id}"),"relative_path":relative.to_string_lossy(),
                "mime_type":"video/mp4","sha256":report.output_sha256,"size_bytes":metadata.len()}]
        }),
        report.master_loudness.as_ref(),
    );
    write_ready_job(jobs, id, &ready)?;
    // The renderer already hashed and decoded this stage. Keep the cancellation
    // gate limited to publication and durable completion, without re-reading a
    // potentially large video while requests are waiting for this gate.
    fs::hard_link(&stage, &output).map_err(|error| {
        CommandError::new(
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                "OVERWRITE_DENIED"
            } else {
                "INTERNAL_ERROR"
            },
            error.to_string(),
        )
    })?;
    write_json_new(
        &jobs.join(format!("{id}.result.json")),
        &json!({"job":ready}),
    )?;
    *job = ready;
    let _ = fs::remove_file(&stage);
    Ok(())
}

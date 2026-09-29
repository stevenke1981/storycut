use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::model::{Asset, Canvas, MAX_REVISION, Project, Subtitle};
use crate::transaction::{
    ApplyResult, Operation, Transaction, apply_operations, changed_and_removed_ids,
};
use crate::validation::{CoreError, CoreErrorCode, validate_project};

const STATE_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PersistedState {
    state_version: u32,
    project: Project,
    project_path: PathBuf,
    create_fingerprint: String,
    idempotency: Vec<IdempotencyRecord>,
    history: Vec<HistoryRecord>,
    #[serde(default)]
    undo_stack: Vec<UndoFrame>,
    #[serde(default)]
    redo_stack: Vec<UndoFrame>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct IdempotencyRecord {
    key: String,
    payload: String,
    result: ApplyResult,
    #[serde(default)]
    response_data: Option<Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoryRecord {
    revision: u64,
    key: String,
    changed_ids: Vec<String>,
    removed_ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct UndoFrame {
    before: Project,
    after: Project,
}

/// Durable project handle. Each successful write refreshes the schema-compatible
/// project file and atomically commits revision, transaction history, and keys
/// together in a workspace-local state file.
#[derive(Clone, Debug)]
pub struct ProjectStore {
    workspace: PathBuf,
    state_path: PathBuf,
}

pub type StoreError = CoreError;

/// Result of a high-level command planned and committed under the project lock.
/// `data` is persisted with the idempotency record so retries can return the
/// original plan summary after later edits have advanced the project revision.
#[derive(Clone, Debug, PartialEq)]
pub struct PlannedApplyResult {
    pub result: ApplyResult,
    pub data: Value,
}

impl ProjectStore {
    /// Create a project under `workspace`, exporting the contract JSON to the
    /// user-selected `project_path`. The create key deterministically identifies
    /// the new project so retries resolve to the same persisted state.
    pub fn create(
        workspace: impl AsRef<Path>,
        project_path: impl AsRef<Path>,
        name: &str,
        canvas: Canvas,
        audio_sample_rate: u32,
        idempotency_key: &str,
    ) -> Result<Self, CoreError> {
        validate_key(idempotency_key)?;
        let workspace = canonical_workspace(workspace.as_ref())?;
        let project_path = resolve_project_path(&workspace, project_path.as_ref())?;
        let request = json!({
            "project_path": project_path,
            "name": name,
            "canvas": canvas,
            "audio_sample_rate": audio_sample_rate,
        });
        let fingerprint = digest_json(&request)?;
        let project_id = format!("p-{}", digest_bytes(idempotency_key.as_bytes()));
        let store = Self::at_workspace_project(&workspace, &project_id)?;

        if store.state_path.exists() {
            let _lock = store.acquire_lock()?;
            let existing = store.read_state_unlocked()?;
            if existing.create_fingerprint != fingerprint {
                return Err(CoreError::new(
                    CoreErrorCode::IdempotencyConflict,
                    "create key was already used with different project data",
                ));
            }
            if existing.project.project_id != project_id
                || normalize_path(&existing.project_path)? != project_path
            {
                return Err(CoreError::new(
                    CoreErrorCode::IdempotencyConflict,
                    "create key resolves to a different project path",
                ));
            }
            store.export_project_unlocked(&existing.project)?;
            return Ok(store);
        }
        if project_path.exists() {
            return Err(CoreError::new(
                CoreErrorCode::OverwriteDenied,
                "project output already exists",
            ));
        }

        let project = Project::empty(project_id, name, canvas, audio_sample_rate);
        validate_project(&project).map_err(|e| CoreError::validation(e.message))?;
        let state = PersistedState {
            state_version: STATE_VERSION,
            project: project.clone(),
            project_path: project_path.clone(),
            create_fingerprint: fingerprint,
            idempotency: Vec::new(),
            history: Vec::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        };
        let _lock = store.acquire_lock()?;
        if store.state_path.exists() || project_path.exists() {
            return Err(CoreError::new(
                CoreErrorCode::OverwriteDenied,
                "project path was created concurrently",
            ));
        }
        store.write_state_unlocked(&state, false)?;
        if let Err(error) = store.export_project_unlocked(&project) {
            // The canonical state remains recoverable. A later open/snapshot repairs
            // the exchange file from that committed state.
            return Err(error);
        }
        Ok(store)
    }

    /// Open a contract project JSON file and locate its sidecar only by a
    /// validated project ID under an ancestor workspace's `projects` directory.
    pub fn open(project_path: impl AsRef<Path>) -> Result<Self, CoreError> {
        let project_path = fs::canonicalize(project_path.as_ref()).map_err(io_error)?;
        let project = read_project(&project_path)?;
        validate_project(&project).map_err(|e| CoreError::validation(e.message))?;
        validate_project_id(&project.project_id)?;
        let mut ancestor = project_path.parent();
        while let Some(workspace) = ancestor {
            if let Ok(canonical) = fs::canonicalize(workspace) {
                let Ok(candidate) =
                    Self::existing_workspace_project(&canonical, &project.project_id)
                else {
                    ancestor = workspace.parent();
                    continue;
                };
                if candidate.state_path.is_file() {
                    let _lock = candidate.acquire_lock()?;
                    let state = candidate.read_state_unlocked()?;
                    if state.project.project_id == project.project_id
                        && normalize_path(&state.project_path)? == project_path
                    {
                        candidate.export_project_unlocked(&state.project)?;
                        return Ok(candidate);
                    }
                }
            }
            ancestor = workspace.parent();
        }
        Err(CoreError::new(
            CoreErrorCode::NotFound,
            "project state sidecar was not found for this project file",
        ))
    }

    /// Resolve a project by workspace and ID. The ID is validated before it is
    /// joined to the fixed `workspace/projects` location.
    pub fn load(workspace: impl AsRef<Path>, project_id: &str) -> Result<Self, CoreError> {
        validate_project_id(project_id)?;
        let workspace = canonical_workspace(workspace.as_ref())?;
        let store = Self::existing_workspace_project(&workspace, project_id)?;
        let _lock = store.acquire_lock()?;
        let state = store.read_state_unlocked()?;
        if state.project.project_id != project_id {
            return Err(CoreError::new(
                CoreErrorCode::Conflict,
                "state sidecar project ID does not match its filename",
            ));
        }
        ensure_project_path_in_workspace(&workspace, &state.project_path)?;
        store.export_project_unlocked(&state.project)?;
        Ok(store)
    }

    /// Read the latest durable snapshot. It is refreshed from the single-writer
    /// state file, so multiple handles and processes do not serve stale projects.
    pub fn snapshot(&self) -> Result<Project, CoreError> {
        let _lock = self.acquire_lock()?;
        let state = self.read_state_unlocked()?;
        self.export_project_unlocked(&state.project)?;
        Ok(state.project)
    }

    pub fn project_path(&self) -> Result<PathBuf, CoreError> {
        let _lock = self.acquire_lock()?;
        Ok(self.read_state_unlocked()?.project_path)
    }

    pub fn state_path(&self) -> &Path {
        &self.state_path
    }

    /// Verify the caller's revision and atomically refresh the contract JSON
    /// projection. Project transactions are already durable before their reply,
    /// so an explicit save records an idempotent save event without incrementing
    /// the project revision.
    pub fn save(
        &self,
        expected_revision: u64,
        idempotency_key: &str,
    ) -> Result<ApplyResult, CoreError> {
        validate_key(idempotency_key)?;
        let payload =
            serde_json::to_string(&json!({"action":"save"})).map_err(serialization_error)?;
        let _lock = self.acquire_lock()?;
        let mut state = self.read_state_unlocked()?;
        if let Some(record) = state
            .idempotency
            .iter()
            .find(|item| item.key == idempotency_key)
        {
            if record.payload == payload {
                return Ok(self.replay_result_unlocked(&state, record));
            }
            return Err(CoreError::new(
                CoreErrorCode::IdempotencyConflict,
                "idempotency key was already used with a different action",
            ));
        }
        if expected_revision != state.project.revision {
            return Err(CoreError::new(
                CoreErrorCode::RevisionConflict,
                format!(
                    "expected revision {expected_revision}, current revision is {}",
                    state.project.revision
                ),
            ));
        }
        self.export_project_unlocked(&state.project)?;
        let revision = state.project.revision;
        let result = ApplyResult {
            applied: true,
            dry_run: false,
            base_revision: revision,
            revision,
            changed_ids: Vec::new(),
            removed_ids: Vec::new(),
            duration_ticks: state.project.duration_ticks(),
            projection_warning: None,
        };
        state.idempotency.push(IdempotencyRecord {
            key: idempotency_key.to_owned(),
            payload,
            result: result.clone(),
            response_data: None,
        });
        state.history.push(HistoryRecord {
            revision,
            key: idempotency_key.to_owned(),
            changed_ids: Vec::new(),
            removed_ids: Vec::new(),
        });
        self.write_state_unlocked(&state, true)?;
        Ok(result)
    }

    /// Commit a batch against the current revision. Canonical idempotency lookup
    /// precedes revision checking; dry runs never reserve keys or write state.
    pub fn apply(
        &self,
        expected_revision: u64,
        idempotency_key: &str,
        dry_run: bool,
        operations: Vec<Operation>,
    ) -> Result<ApplyResult, CoreError> {
        // The sidecar is canonical; determining its project ID should not first
        // require the exchange JSON projection to be writable.
        let project_id = {
            let _lock = self.acquire_lock()?;
            self.read_state_unlocked()?.project.project_id
        };
        self.transact(Transaction {
            project_id,
            expected_revision,
            idempotency_key: idempotency_key.to_owned(),
            dry_run,
            operations,
        })
    }

    pub fn transact(&self, transaction: Transaction) -> Result<ApplyResult, CoreError> {
        validate_key(&transaction.idempotency_key)?;
        if transaction.operations.is_empty() || transaction.operations.len() > 500 {
            return Err(CoreError::validation(
                "a transaction must contain 1 to 500 operations",
            ));
        }
        let payload =
            serde_json::to_string(&transaction.operations).map_err(serialization_error)?;
        self.transact_planned(
            transaction.project_id,
            transaction.expected_revision,
            transaction.idempotency_key,
            transaction.dry_run,
            payload,
            move |_| Ok((transaction.operations, Value::Null)),
        )
        .map(|planned| planned.result)
    }

    /// Plan and apply one high-level action while holding the same durable
    /// project lock used for revision, history, undo, and idempotency updates.
    /// The identity must contain only semantic request fields; callers omit
    /// `expected_revision`, `dry_run`, and `idempotency_key` so retries replay
    /// after later revisions just like primitive timeline transactions.
    pub fn apply_planned<F>(
        &self,
        expected_revision: u64,
        idempotency_key: &str,
        dry_run: bool,
        command: &str,
        identity: Value,
        planner: F,
    ) -> Result<PlannedApplyResult, CoreError>
    where
        F: FnOnce(&Project) -> Result<(Vec<Operation>, Value), CoreError>,
    {
        validate_key(idempotency_key)?;
        let project_id = {
            let _lock = self.acquire_lock()?;
            self.read_state_unlocked()?.project.project_id
        };
        let payload = serde_json::to_string(&json!({
            "command": command,
            "request": identity,
        }))
        .map_err(serialization_error)?;
        self.transact_planned(
            project_id,
            expected_revision,
            idempotency_key.to_owned(),
            dry_run,
            payload,
            planner,
        )
    }

    fn transact_planned<F>(
        &self,
        project_id: String,
        expected_revision: u64,
        idempotency_key: String,
        dry_run: bool,
        payload: String,
        planner: F,
    ) -> Result<PlannedApplyResult, CoreError>
    where
        F: FnOnce(&Project) -> Result<(Vec<Operation>, Value), CoreError>,
    {
        validate_key(&idempotency_key)?;
        let _lock = self.acquire_lock()?;
        let mut state = self.read_state_unlocked()?;
        if project_id != state.project.project_id {
            return Err(CoreError::new(
                CoreErrorCode::Conflict,
                "transaction project_id does not match the opened project",
            ));
        }
        if let Some(record) = state
            .idempotency
            .iter()
            .find(|item| item.key == idempotency_key)
        {
            if record.payload == payload {
                return Ok(PlannedApplyResult {
                    result: self.replay_result_unlocked(&state, record),
                    data: record.response_data.clone().unwrap_or(Value::Null),
                });
            }
            return Err(CoreError::new(
                CoreErrorCode::IdempotencyConflict,
                "idempotency key was already used with a different action",
            ));
        }
        if expected_revision != state.project.revision {
            return Err(CoreError::new(
                CoreErrorCode::RevisionConflict,
                format!(
                    "expected revision {}, current revision is {}",
                    expected_revision, state.project.revision
                ),
            ));
        }
        let (operations, response_data) = planner(&state.project)?;
        if operations.is_empty() || operations.len() > 500 {
            return Err(CoreError::validation(
                "a transaction must contain 1 to 500 operations",
            ));
        }
        let base_revision = state.project.revision;
        let before = state.project.clone();
        let mut candidate = before.clone();
        let locked = state
            .project
            .tracks
            .iter()
            .filter(|track| track.locked)
            .map(|track| track.id.clone())
            .collect();
        apply_operations(&mut candidate, &operations, &locked)?;
        let (changed_ids, removed_ids) = changed_and_removed_ids(&state.project, &candidate)?;
        if dry_run {
            return Ok(PlannedApplyResult {
                result: ApplyResult {
                    applied: false,
                    dry_run: true,
                    base_revision,
                    revision: base_revision,
                    changed_ids,
                    removed_ids,
                    duration_ticks: candidate.duration_ticks(),
                    projection_warning: None,
                },
                data: response_data,
            });
        }
        let next_revision = base_revision
            .checked_add(1)
            .filter(|revision| *revision <= MAX_REVISION)
            .ok_or_else(|| {
                CoreError::validation("project revision reached the safe integer limit")
            })?;
        candidate.revision = next_revision;
        validate_project(&candidate).map_err(|e| CoreError::validation(e.message))?;
        let mut result = ApplyResult {
            applied: true,
            dry_run: false,
            base_revision,
            revision: next_revision,
            changed_ids: changed_ids.clone(),
            removed_ids: removed_ids.clone(),
            duration_ticks: candidate.duration_ticks(),
            projection_warning: None,
        };
        state.project = candidate.clone();
        state.undo_stack.push(UndoFrame {
            before,
            after: candidate,
        });
        state.redo_stack.clear();
        state.idempotency.push(IdempotencyRecord {
            key: idempotency_key.clone(),
            payload,
            result: result.clone(),
            response_data: Some(response_data.clone()),
        });
        state.history.push(HistoryRecord {
            revision: next_revision,
            key: idempotency_key,
            changed_ids,
            removed_ids,
        });
        result.projection_warning = self.commit_state_and_export(&state)?;
        Ok(PlannedApplyResult {
            result,
            data: response_data,
        })
    }

    pub fn undo(
        &self,
        expected_revision: u64,
        idempotency_key: &str,
        dry_run: bool,
    ) -> Result<ApplyResult, CoreError> {
        self.apply_history_action(expected_revision, idempotency_key, dry_run, false)
    }

    pub fn redo(
        &self,
        expected_revision: u64,
        idempotency_key: &str,
        dry_run: bool,
    ) -> Result<ApplyResult, CoreError> {
        self.apply_history_action(expected_revision, idempotency_key, dry_run, true)
    }

    fn apply_history_action(
        &self,
        expected_revision: u64,
        idempotency_key: &str,
        dry_run: bool,
        redo: bool,
    ) -> Result<ApplyResult, CoreError> {
        validate_key(idempotency_key)?;
        let action = if redo { "redo" } else { "undo" };
        let payload =
            serde_json::to_string(&json!({"action": action})).map_err(serialization_error)?;
        let _lock = self.acquire_lock()?;
        let mut state = self.read_state_unlocked()?;
        if let Some(record) = state
            .idempotency
            .iter()
            .find(|item| item.key == idempotency_key)
        {
            if record.payload == payload {
                return Ok(self.replay_result_unlocked(&state, record));
            }
            return Err(CoreError::new(
                CoreErrorCode::IdempotencyConflict,
                "idempotency key was already used with a different action",
            ));
        }
        if expected_revision != state.project.revision {
            return Err(CoreError::new(
                CoreErrorCode::RevisionConflict,
                format!(
                    "expected revision {expected_revision}, current revision is {}",
                    state.project.revision
                ),
            ));
        }
        let frame = if redo {
            state.redo_stack.last()
        } else {
            state.undo_stack.last()
        }
        .cloned()
        .ok_or_else(|| CoreError::new(CoreErrorCode::Conflict, format!("nothing to {action}")))?;
        let before = state.project.clone();
        let mut candidate = if redo {
            frame.after.clone()
        } else {
            frame.before.clone()
        };
        let (changed_ids, removed_ids) = changed_and_removed_ids(&before, &candidate)?;
        let base_revision = before.revision;
        if dry_run {
            return Ok(ApplyResult {
                applied: false,
                dry_run: true,
                base_revision,
                revision: base_revision,
                changed_ids,
                removed_ids,
                duration_ticks: candidate.duration_ticks(),
                projection_warning: None,
            });
        }
        candidate.revision = base_revision
            .checked_add(1)
            .filter(|revision| *revision <= MAX_REVISION)
            .ok_or_else(|| {
                CoreError::validation("project revision reached the safe integer limit")
            })?;
        validate_project(&candidate).map_err(|error| CoreError::validation(error.message))?;
        let mut result = ApplyResult {
            applied: true,
            dry_run: false,
            base_revision,
            revision: candidate.revision,
            changed_ids: changed_ids.clone(),
            removed_ids: removed_ids.clone(),
            duration_ticks: candidate.duration_ticks(),
            projection_warning: None,
        };
        if redo {
            state.redo_stack.pop();
            state.undo_stack.push(frame);
        } else {
            state.undo_stack.pop();
            state.redo_stack.push(frame);
        }
        state.project = candidate;
        state.history.push(HistoryRecord {
            revision: result.revision,
            key: idempotency_key.to_owned(),
            changed_ids,
            removed_ids,
        });
        state.idempotency.push(IdempotencyRecord {
            key: idempotency_key.to_owned(),
            payload,
            result: result.clone(),
            response_data: None,
        });
        result.projection_warning = self.commit_state_and_export(&state)?;
        Ok(result)
    }

    /// Add media records through the same revision, lock, idempotency, history,
    /// and durable transaction path as timeline operations.
    pub fn import_assets(
        &self,
        expected_revision: u64,
        idempotency_key: &str,
        dry_run: bool,
        assets: Vec<Asset>,
    ) -> Result<ApplyResult, CoreError> {
        validate_key(idempotency_key)?;
        if assets.is_empty() || assets.len() > 500 {
            return Err(CoreError::validation(
                "media import must contain 1 to 500 assets",
            ));
        }
        let _lock = self.acquire_lock()?;
        let mut state = self.read_state_unlocked()?;
        let payload = serde_json::to_string(&json!({"kind":"media_import","assets":&assets}))
            .map_err(serialization_error)?;
        if let Some(record) = state
            .idempotency
            .iter()
            .find(|item| item.key == idempotency_key)
        {
            if record.payload == payload {
                return Ok(self.replay_result_unlocked(&state, record));
            }
            return Err(CoreError::new(
                CoreErrorCode::IdempotencyConflict,
                "idempotency key was already used with different media",
            ));
        }
        if expected_revision != state.project.revision {
            return Err(CoreError::new(
                CoreErrorCode::RevisionConflict,
                format!(
                    "expected revision {expected_revision}, current revision is {}",
                    state.project.revision
                ),
            ));
        }
        let base_revision = state.project.revision;
        let mut candidate = state.project.clone();
        candidate.assets.extend(assets);
        validate_project(&candidate).map_err(|e| CoreError::validation(e.message))?;
        let before = state.project.clone();
        let (changed_ids, removed_ids) = changed_and_removed_ids(&before, &candidate)?;
        if dry_run {
            return Ok(ApplyResult {
                applied: false,
                dry_run: true,
                base_revision,
                revision: base_revision,
                changed_ids,
                removed_ids,
                duration_ticks: candidate.duration_ticks(),
                projection_warning: None,
            });
        }
        let next_revision = base_revision
            .checked_add(1)
            .filter(|v| *v <= MAX_REVISION)
            .ok_or_else(|| {
                CoreError::validation("project revision reached the safe integer limit")
            })?;
        candidate.revision = next_revision;
        let mut result = ApplyResult {
            applied: true,
            dry_run: false,
            base_revision,
            revision: next_revision,
            changed_ids: changed_ids.clone(),
            removed_ids: removed_ids.clone(),
            duration_ticks: candidate.duration_ticks(),
            projection_warning: None,
        };
        state.project = candidate.clone();
        state.undo_stack.push(UndoFrame {
            before,
            after: candidate,
        });
        state.redo_stack.clear();
        state.idempotency.push(IdempotencyRecord {
            key: idempotency_key.to_owned(),
            payload,
            result: result.clone(),
            response_data: None,
        });
        state.history.push(HistoryRecord {
            revision: next_revision,
            key: idempotency_key.to_owned(),
            changed_ids,
            removed_ids,
        });
        result.projection_warning = self.commit_state_and_export(&state)?;
        Ok(result)
    }

    pub fn import_subtitles(
        &self,
        expected_revision: u64,
        idempotency_key: &str,
        dry_run: bool,
        subtitles: Vec<Subtitle>,
    ) -> Result<ApplyResult, CoreError> {
        validate_key(idempotency_key)?;
        if subtitles.is_empty() || subtitles.len() > 500 {
            return Err(CoreError::validation(
                "subtitle import must contain 1 to 500 documents",
            ));
        }
        let _lock = self.acquire_lock()?;
        let mut state = self.read_state_unlocked()?;
        let payload =
            serde_json::to_string(&json!({"kind":"subtitle_import","subtitles":&subtitles}))
                .map_err(serialization_error)?;
        if let Some(record) = state
            .idempotency
            .iter()
            .find(|item| item.key == idempotency_key)
        {
            if record.payload == payload {
                return Ok(self.replay_result_unlocked(&state, record));
            }
            return Err(CoreError::new(
                CoreErrorCode::IdempotencyConflict,
                "idempotency key was already used with different subtitle documents",
            ));
        }
        if expected_revision != state.project.revision {
            return Err(CoreError::new(
                CoreErrorCode::RevisionConflict,
                format!(
                    "expected revision {expected_revision}, current revision is {}",
                    state.project.revision
                ),
            ));
        }
        for subtitle in &subtitles {
            let track = state
                .project
                .tracks
                .iter()
                .find(|track| track.id == subtitle.track_id)
                .ok_or_else(|| {
                    CoreError::new(
                        CoreErrorCode::NotFound,
                        format!("track {} was not found", subtitle.track_id),
                    )
                })?;
            if track.locked {
                return Err(CoreError::new(
                    CoreErrorCode::LockedTrack,
                    format!("track {} is locked", track.id),
                ));
            }
        }
        let base_revision = state.project.revision;
        let before = state.project.clone();
        let mut candidate = before.clone();
        candidate.subtitles.extend(subtitles);
        validate_project(&candidate).map_err(|error| CoreError::validation(error.message))?;
        let (changed_ids, removed_ids) = changed_and_removed_ids(&before, &candidate)?;
        if dry_run {
            return Ok(ApplyResult {
                applied: false,
                dry_run: true,
                base_revision,
                revision: base_revision,
                changed_ids,
                removed_ids,
                duration_ticks: candidate.duration_ticks(),
                projection_warning: None,
            });
        }
        let revision = base_revision
            .checked_add(1)
            .filter(|value| *value <= MAX_REVISION)
            .ok_or_else(|| {
                CoreError::validation("project revision reached the safe integer limit")
            })?;
        candidate.revision = revision;
        let mut result = ApplyResult {
            applied: true,
            dry_run: false,
            base_revision,
            revision,
            changed_ids: changed_ids.clone(),
            removed_ids: removed_ids.clone(),
            duration_ticks: candidate.duration_ticks(),
            projection_warning: None,
        };
        state.project = candidate.clone();
        state.undo_stack.push(UndoFrame {
            before,
            after: candidate,
        });
        state.redo_stack.clear();
        state.idempotency.push(IdempotencyRecord {
            key: idempotency_key.to_owned(),
            payload,
            result: result.clone(),
            response_data: None,
        });
        state.history.push(HistoryRecord {
            revision,
            key: idempotency_key.to_owned(),
            changed_ids,
            removed_ids,
        });
        result.projection_warning = self.commit_state_and_export(&state)?;
        Ok(result)
    }

    fn at_workspace_project(workspace: &Path, project_id: &str) -> Result<Self, CoreError> {
        validate_project_id(project_id)?;
        let projects_dir = workspace.join("projects");
        fs::create_dir_all(&projects_dir).map_err(io_error)?;
        let canonical_projects = fs::canonicalize(&projects_dir).map_err(io_error)?;
        if !canonical_projects.starts_with(workspace) {
            return Err(CoreError::new(
                CoreErrorCode::IoError,
                "projects directory escapes the workspace",
            ));
        }
        let state_path = canonical_projects.join(format!("{project_id}.storycut-state.json"));
        Ok(Self {
            workspace: workspace.to_path_buf(),
            state_path,
        })
    }

    fn existing_workspace_project(workspace: &Path, project_id: &str) -> Result<Self, CoreError> {
        validate_project_id(project_id)?;
        let projects_dir = workspace.join("projects");
        let metadata = fs::symlink_metadata(&projects_dir).map_err(io_error)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(CoreError::new(
                CoreErrorCode::PathDenied,
                "workspace projects directory is not a regular directory",
            ));
        }
        let canonical_projects = fs::canonicalize(&projects_dir).map_err(io_error)?;
        if !canonical_projects.starts_with(workspace) {
            return Err(CoreError::new(
                CoreErrorCode::PathDenied,
                "projects directory escapes the workspace",
            ));
        }
        Ok(Self {
            workspace: workspace.to_path_buf(),
            state_path: canonical_projects.join(format!("{project_id}.storycut-state.json")),
        })
    }

    fn acquire_lock(&self) -> Result<File, CoreError> {
        let lock_path = self.state_path.with_extension("lock");
        if let Some(parent) = lock_path.parent() {
            fs::create_dir_all(parent).map_err(io_error)?;
        }
        if fs::symlink_metadata(&lock_path).is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            return Err(CoreError::new(
                CoreErrorCode::PathDenied,
                "state lock must not be a symbolic link",
            ));
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(lock_path)
            .map_err(io_error)?;
        FileExt::lock_exclusive(&file).map_err(io_error)?;
        Ok(file)
    }

    fn read_state_unlocked(&self) -> Result<PersistedState, CoreError> {
        let metadata = fs::symlink_metadata(&self.state_path).map_err(io_error)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(CoreError::new(
                CoreErrorCode::IoError,
                "project state must be a regular file",
            ));
        }
        let mut file = File::open(&self.state_path).map_err(io_error)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).map_err(io_error)?;
        let state: PersistedState = serde_json::from_slice(&bytes).map_err(serialization_error)?;
        if state.state_version != STATE_VERSION {
            return Err(CoreError::new(
                CoreErrorCode::UnsupportedFeature,
                "project state version is unsupported",
            ));
        }
        validate_project(&state.project).map_err(|e| CoreError::validation(e.message))?;
        validate_project_id(&state.project.project_id)?;
        if state.project.project_id
            != self
                .state_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .trim_end_matches(".storycut-state.json")
        {
            return Err(CoreError::new(
                CoreErrorCode::Conflict,
                "project ID does not match its state file name",
            ));
        }
        ensure_project_path_in_workspace(&self.workspace, &state.project_path)?;
        Ok(state)
    }

    fn write_state_unlocked(&self, state: &PersistedState, replace: bool) -> Result<(), CoreError> {
        let bytes = serde_json::to_vec_pretty(state).map_err(serialization_error)?;
        atomic_write(&self.state_path, &bytes, replace)
    }

    fn export_project_unlocked(&self, project: &Project) -> Result<(), CoreError> {
        let state = self.read_state_unlocked()?;
        let project_path = ensure_project_path_in_workspace(&self.workspace, &state.project_path)?;
        let bytes = serde_json::to_vec_pretty(project).map_err(serialization_error)?;
        atomic_write(&project_path, &bytes, true)
    }

    fn replay_result_unlocked(
        &self,
        state: &PersistedState,
        record: &IdempotencyRecord,
    ) -> ApplyResult {
        let mut result = record.result.clone();
        result.projection_warning = self
            .export_project_unlocked(&state.project)
            .err()
            .map(|error| projection_warning(state.project.revision, &error));
        result
    }

    fn commit_state_and_export(&self, state: &PersistedState) -> Result<Option<String>, CoreError> {
        // The sidecar is the canonical commit point. Once this succeeds, an
        // exchange-file failure must not make callers believe the revision was
        // rolled back. Return it as a warning; snapshot/open and idempotent
        // retries will attempt to refresh the projection again.
        self.write_state_unlocked(state, true)?;
        Ok(self
            .export_project_unlocked(&state.project)
            .err()
            .map(|error| projection_warning(state.project.revision, &error)))
    }
}

fn projection_warning(revision: u64, error: &CoreError) -> String {
    format!(
        "revision {revision} committed to canonical state, but the project JSON export is pending: {error}"
    )
}

fn canonical_workspace(workspace: &Path) -> Result<PathBuf, CoreError> {
    fs::create_dir_all(workspace).map_err(io_error)?;
    let canonical = fs::canonicalize(workspace).map_err(io_error)?;
    if !canonical.is_dir() {
        return Err(CoreError::new(
            CoreErrorCode::IoError,
            "workspace is not a directory",
        ));
    }
    Ok(canonical)
}

fn resolve_project_path(workspace: &Path, input: &Path) -> Result<PathBuf, CoreError> {
    let joined = if input.is_absolute() {
        input.to_path_buf()
    } else {
        workspace.join(input)
    };
    let name = joined
        .file_name()
        .ok_or_else(|| CoreError::validation("project path has no file name"))?;
    let parent = joined
        .parent()
        .ok_or_else(|| CoreError::validation("project path has no parent"))?;
    let canonical_parent = fs::canonicalize(parent).map_err(io_error)?;
    if !canonical_parent.starts_with(workspace) {
        return Err(CoreError::new(
            CoreErrorCode::PathDenied,
            "project path is outside the authorized workspace",
        ));
    }
    Ok(canonical_parent.join(name))
}

fn ensure_project_path_in_workspace(workspace: &Path, path: &Path) -> Result<PathBuf, CoreError> {
    let name = path
        .file_name()
        .ok_or_else(|| CoreError::validation("stored project path has no file name"))?;
    let parent = path
        .parent()
        .ok_or_else(|| CoreError::validation("stored project path has no parent"))?;
    let canonical_parent = fs::canonicalize(parent).map_err(io_error)?;
    if !canonical_parent.starts_with(workspace) {
        return Err(CoreError::new(
            CoreErrorCode::PathDenied,
            "stored project path escapes workspace",
        ));
    }
    let normalized = canonical_parent.join(name);
    if let Ok(canonical_file) = fs::canonicalize(&normalized) {
        if canonical_file != normalized {
            return Err(CoreError::new(
                CoreErrorCode::PathDenied,
                "project file resolves through an unexpected link",
            ));
        }
    }
    Ok(normalized)
}

fn normalize_path(path: &Path) -> Result<PathBuf, CoreError> {
    if path.exists() {
        fs::canonicalize(path).map_err(io_error)
    } else {
        let parent = path
            .parent()
            .ok_or_else(|| CoreError::validation("path has no parent"))?;
        let name = path
            .file_name()
            .ok_or_else(|| CoreError::validation("path has no file name"))?;
        Ok(fs::canonicalize(parent).map_err(io_error)?.join(name))
    }
}

fn validate_project_id(id: &str) -> Result<(), CoreError> {
    let b = id.as_bytes();
    if b.is_empty()
        || b.len() > 80
        || !b[0].is_ascii_alphanumeric()
        || !b
            .iter()
            .all(|v| v.is_ascii_alphanumeric() || matches!(v, b'_' | b'.' | b'-'))
    {
        return Err(CoreError::validation(
            "project_id has an invalid identifier",
        ));
    }
    Ok(())
}

fn validate_key(key: &str) -> Result<(), CoreError> {
    if key.trim().is_empty() || key.len() > 200 {
        return Err(CoreError::validation(
            "idempotency_key must contain 1 to 200 characters",
        ));
    }
    Ok(())
}

fn read_project(path: &Path) -> Result<Project, CoreError> {
    let bytes = fs::read(path).map_err(io_error)?;
    serde_json::from_slice(&bytes).map_err(serialization_error)
}

fn digest_json(value: &serde_json::Value) -> Result<String, CoreError> {
    let bytes = serde_json::to_vec(value).map_err(serialization_error)?;
    Ok(digest_bytes(&bytes))
}
fn digest_bytes(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn atomic_write(path: &Path, bytes: &[u8], replace: bool) -> Result<(), CoreError> {
    let parent = path
        .parent()
        .ok_or_else(|| CoreError::validation("atomic output has no parent"))?;
    let temporary = parent.join(format!(".storycut-{}.tmp", Uuid::new_v4().simple()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(io_error)?;
    let result = (|| {
        file.write_all(bytes).map_err(io_error)?;
        file.sync_all().map_err(io_error)?;
        drop(file);
        if !replace && path.exists() {
            return Err(CoreError::new(
                CoreErrorCode::OverwriteDenied,
                "output already exists",
            ));
        }
        fs::rename(&temporary, path).map_err(io_error)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn io_error(error: std::io::Error) -> CoreError {
    let code = match error.kind() {
        std::io::ErrorKind::NotFound => CoreErrorCode::NotFound,
        std::io::ErrorKind::AlreadyExists => CoreErrorCode::OverwriteDenied,
        std::io::ErrorKind::PermissionDenied => CoreErrorCode::PathDenied,
        _ => CoreErrorCode::IoError,
    };
    CoreError::new(code, error.to_string())
}
fn serialization_error(error: serde_json::Error) -> CoreError {
    CoreError::new(CoreErrorCode::SerializationError, error.to_string())
}

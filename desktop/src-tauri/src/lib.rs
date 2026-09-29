use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::sync::Mutex;
use sha2::{Digest, Sha256};
use tauri::State;
use uuid::Uuid;

mod media_thumbnail;

struct SelectedWorkspace(Mutex<Option<PathBuf>>);
const MAX_PREVIEW_BYTES: u64 = 128 * 1024 * 1024;

#[tauri::command]
async fn storycut_read_asset_thumbnail(
    project_id: String,
    asset_id: String,
    state: State<'_, SelectedWorkspace>,
) -> Result<Value, String> {
    let workspace = state
        .0
        .lock()
        .map_err(|_| "工作區狀態無法使用。".to_owned())?
        .clone()
        .ok_or_else(|| "請先開啟或建立專案。".to_owned())?;
    tauri::async_runtime::spawn_blocking(move || {
        media_thumbnail::read(&workspace, &project_id, &asset_id)
    })
    .await
    .map_err(|error| format!("來源縮圖工作失敗：{error}"))?
}

#[tauri::command]
fn storycut_set_workspace(path: String, state: State<'_, SelectedWorkspace>) -> Result<(), String> {
    let candidate = PathBuf::from(path);
    let canonical = std::fs::canonicalize(&candidate).map_err(|error| format!("無法開啟工作區：{error}"))?;
    if !canonical.is_dir() {
        return Err("工作區必須是已存在的資料夾。".to_owned());
    }
    *state.0.lock().map_err(|_| "工作區狀態無法使用。".to_owned())? = Some(canonical);
    Ok(())
}

#[tauri::command]
async fn storycut_dispatch(tool: String, args: Value, state: State<'_, SelectedWorkspace>) -> Result<Value, String> {
    let workspace = match state.0.lock() {
        Ok(guard) => guard.clone(),
        Err(_) => None,
    };
    let Some(workspace) = workspace else {
        return Ok(failure("PATH_DENIED", "請先開啟或建立專案，選擇一個工作區。", json!({})));
    };
    match tauri::async_runtime::spawn_blocking(move || storycut_command::dispatch(&workspace, &tool, args)).await {
        Ok(Ok(response)) => Ok(response),
        Ok(Err(error)) => Ok(failure(&error.code, &error.message, error.details)),
        Err(error) => Ok(failure("INTERNAL_ERROR", &format!("核心指令工作失敗：{error}"), json!({}))),
    }
}

#[tauri::command]
fn storycut_read_preview(job_id: String, artifact_id: String, state: State<'_, SelectedWorkspace>) -> Result<Value, String> {
    let workspace = state.0.lock().map_err(|_| "工作區狀態無法使用。".to_owned())?
        .clone().ok_or_else(|| "請先開啟或建立專案。".to_owned())?;
    let response = storycut_command::dispatch(&workspace, "storycut_job_get", json!({
        "job_id": job_id,
        "include_preview": false
    })).map_err(|error| format!("{}：{}", error.code, error.message))?;
    if response.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err(response.pointer("/error/message").and_then(Value::as_str)
            .unwrap_or("無法讀取預覽工作。").to_owned());
    }
    let artifact = response.pointer("/data/job/artifacts").and_then(Value::as_array)
        .and_then(|items| items.iter().find(|item| item.get("artifact_id").and_then(Value::as_str) == Some(&artifact_id)))
        .ok_or_else(|| "核心工作沒有此預覽產物。".to_owned())?;
    let mime_type = artifact.get("mime_type").and_then(Value::as_str)
        .ok_or_else(|| "預覽產物缺少 MIME 類型。".to_owned())?;
    let kind = response.pointer("/data/job/kind").and_then(Value::as_str)
        .ok_or_else(|| "預覽工作缺少類型。".to_owned())?;
    let extension = match (kind, mime_type) {
        ("preview_frame", "image/png") => "png",
        ("preview_range", "video/mp4") => "mp4",
        _ => return Err("只允許讀取核心產生的 PNG 影格或 MP4 預覽。".to_owned()),
    };
    let relative = artifact.get("relative_path").and_then(Value::as_str)
        .ok_or_else(|| "預覽產物路徑缺失。".to_owned())?;
    let relative_path = std::path::Path::new(relative);
    if relative_path.is_absolute() || relative_path.components().any(|part| matches!(part, std::path::Component::ParentDir)) {
        return Err("預覽產物路徑不合法。".to_owned());
    }
    let workspace = fs::canonicalize(&workspace).map_err(|error| format!("工作區不存在：{error}"))?;
    let previews = fs::canonicalize(workspace.join(".storycut").join("previews"))
        .map_err(|error| format!("預覽目錄尚不存在：{error}"))?;
    let path = fs::canonicalize(workspace.join(relative_path))
        .map_err(|error| format!("預覽影格尚未寫入：{error}"))?;
    if !path.starts_with(&previews) || !path.is_file() || path.extension().and_then(|value| value.to_str()) != Some(extension) {
        return Err("只允許讀取核心產生且位於 workspace/.storycut/previews 的預覽產物。".to_owned());
    }
    let metadata = fs::metadata(&path).map_err(|error| format!("無法檢查預覽產物：{error}"))?;
    if metadata.len() > MAX_PREVIEW_BYTES {
        return Err("預覽產物超過 128 MiB，無法經桌面 IPC 傳送。".to_owned());
    }
    let expected_size = artifact.get("size_bytes").and_then(Value::as_u64)
        .ok_or_else(|| "核心預覽產物缺少大小資訊。".to_owned())?;
    let expected_sha = artifact.get("sha256").and_then(Value::as_str)
        .ok_or_else(|| "核心預覽產物缺少 SHA-256。".to_owned())?;
    if expected_size > MAX_PREVIEW_BYTES {
        return Err("預覽產物超過 128 MiB，無法經桌面 IPC 傳送。".to_owned());
    }
    let file = fs::File::open(&path).map_err(|error| format!("無法讀取預覽影格：{error}"))?;
    let mut bytes = Vec::with_capacity(metadata.len().min(16 * 1024 * 1024) as usize);
    file.take(MAX_PREVIEW_BYTES + 1).read_to_end(&mut bytes)
        .map_err(|error| format!("無法讀取預覽影格：{error}"))?;
    let bytes = verify_preview_bytes(bytes, expected_size, expected_sha, MAX_PREVIEW_BYTES)?;
    Ok(json!({
        "mime_type": mime_type,
        "data_url": format!("data:{mime_type};base64,{}", STANDARD.encode(bytes))
    }))
}

fn verify_preview_bytes(bytes: Vec<u8>, expected_size: u64, expected_sha: &str, max_bytes: u64) -> Result<Vec<u8>, String> {
    if bytes.len() as u64 > max_bytes {
        return Err("預覽產物超過大小限制。".to_owned());
    }
    if bytes.len() as u64 != expected_size {
        return Err("預覽產物大小與核心工作記錄不一致。".to_owned());
    }
    let actual_sha = format!("{:x}", Sha256::digest(&bytes));
    if !actual_sha.eq_ignore_ascii_case(expected_sha) {
        return Err("預覽產物 SHA-256 與核心工作記錄不一致。".to_owned());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_bridge_rejects_changed_size_and_digest() {
        let bytes = b"core preview".to_vec();
        let digest = format!("{:x}", Sha256::digest(&bytes));
        assert!(verify_preview_bytes(bytes.clone(), bytes.len() as u64 + 1, &digest, 1024).is_err());
        assert!(verify_preview_bytes(bytes.clone(), bytes.len() as u64, "00", 1024).is_err());
        assert!(verify_preview_bytes(bytes.clone(), bytes.len() as u64, &digest, 4).is_err());
        assert_eq!(verify_preview_bytes(bytes.clone(), bytes.len() as u64, &digest.to_uppercase(), 1024).unwrap(), bytes);
    }
}

fn failure(code: &str, message: &str, details: Value) -> Value {
    json!({
        "api_version": "0.2.0",
        "ok": false,
        "request_id": Uuid::new_v4().to_string(),
        "project_id": null,
        "revision": null,
        "data": null,
        "error": { "code": code, "message": message, "retryable": false, "details": details },
        "warnings": []
    })
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(SelectedWorkspace(Mutex::new(None)))
        .invoke_handler(tauri::generate_handler![storycut_set_workspace, storycut_dispatch, storycut_read_preview, storycut_read_asset_thumbnail])
        .run(tauri::generate_context!())
        .expect("StoryCut desktop runtime failed");
}

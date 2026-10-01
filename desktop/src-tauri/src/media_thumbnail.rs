//! Read-only, bounded source thumbnails for selecting a subject's focal point.
//! Asset IDs are resolved from the same project store used by CLI and MCP.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use storycut_core::{AssetKind, ProjectStore};

const MAX_PNG_BYTES: u64 = 2 * 1024 * 1024;
const MAX_SOURCE_PIXELS: &str = "16777216";
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

pub fn read(workspace: &Path, project_id: &str, asset_id: &str) -> Result<Value, String> {
    let store = ProjectStore::load(workspace, project_id).map_err(|e| e.to_string())?;
    let project = store.snapshot().map_err(|e| e.to_string())?;
    let asset = project
        .assets
        .iter()
        .find(|a| a.id == asset_id)
        .ok_or_else(|| "專案中沒有指定素材。".to_owned())?;
    if !matches!(asset.kind, AssetKind::Image | AssetKind::Video) {
        return Err("只有圖片與影片可產生來源縮圖。".to_owned());
    }
    let project_path = store.project_path().map_err(|e| e.to_string())?;
    let source = source_path(workspace, &project_path, &asset.path)?;
    let bytes = decode(&source, matches!(asset.kind, AssetKind::Video))?;
    let (width, height) = dimensions(&bytes)?;
    Ok(
        json!({"asset_id":asset_id, "mime_type":"image/png", "width":width, "height":height,
        "data_url":format!("data:image/png;base64,{}", STANDARD.encode(bytes))}),
    )
}

/// Scale filter shared by images and videos. Videos first run `thumbnail`, which
/// picks the most representative frame of the first 48 decoded frames, so a
/// fade-in or black leader does not become the bin thumbnail.
fn thumbnail_filter(video: bool) -> String {
    let scale = "scale=w='min(640,640*dar)':h='min(640,640/dar)',setsar=1";
    if video { format!("thumbnail=48,{scale}") } else { scale.to_owned() }
}

fn decode(source: &Path, video: bool) -> Result<Vec<u8>, String> {
    let filter = thumbnail_filter(video);
    let mut command = Command::new("ffmpeg");
    command
        .args([
            "-v",
            "error",
            "-nostdin",
            "-filter_threads",
            "1",
            "-threads",
            "1",
            "-max_pixels",
            MAX_SOURCE_PIXELS,
            "-i",
        ])
        .arg(&source)
        .args([
            "-map",
            "0:v:0",
            "-vf",
        ])
        .arg(&filter)
        .args([
            "-frames:v",
            "1",
            "-c:v",
            "png",
            "-threads",
            "1",
            "-f",
            "image2pipe",
            "pipe:1",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW for the decoder helper.
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("無法啟動 FFmpeg 縮圖：{e}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "FFmpeg 縮圖輸出不可用。".to_owned())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "FFmpeg 縮圖錯誤輸出不可用。".to_owned())?;
    let reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(MAX_PNG_BYTES + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let errors = thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr
            .take(64 * 1024)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let deadline = Instant::now() + Duration::from_secs(15);
    let completion = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(40)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err("來源縮圖超過 15 秒，請檢查素材。".to_owned());
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(format!("縮圖程序失敗：{error}"));
            }
        }
    };
    let bytes = reader
        .join()
        .map_err(|_| "縮圖讀取程序失敗。".to_owned())?
        .map_err(|e| format!("無法讀取縮圖：{e}"))?;
    let stderr = errors
        .join()
        .map_err(|_| "縮圖診斷程序失敗。".to_owned())?
        .map_err(|e| format!("無法讀取縮圖診斷：{e}"))?;
    let status = completion?;
    if !status.success() {
        return Err(format!(
            "無法解碼來源縮圖：{}",
            String::from_utf8_lossy(&stderr)
                .chars()
                .take(500)
                .collect::<String>()
        ));
    }
    dimensions(&bytes)?;
    Ok(bytes)
}

fn source_path(workspace: &Path, project: &Path, source: &str) -> Result<PathBuf, String> {
    let root = fs::canonicalize(workspace).map_err(|e| format!("工作區不存在：{e}"))?;
    let project = fs::canonicalize(project).map_err(|e| format!("專案不存在：{e}"))?;
    if !project.starts_with(&root) {
        return Err("PATH_DENIED：專案不在授權工作區。".to_owned());
    }
    let source = Path::new(source);
    let candidate = if source.is_absolute() {
        source.to_path_buf()
    } else {
        project
            .parent()
            .ok_or_else(|| "專案路徑無父目錄。".to_owned())?
            .join(source)
    };
    let canonical = fs::canonicalize(candidate).map_err(|e| format!("來源素材不存在：{e}"))?;
    if !canonical.starts_with(&root) || !canonical.is_file() {
        return Err("PATH_DENIED：來源縮圖只能讀取授權工作區的素材。".to_owned());
    }
    Ok(canonical)
}

fn dimensions(bytes: &[u8]) -> Result<(u32, u32), String> {
    if bytes.len() > MAX_PNG_BYTES as usize
        || bytes.len() < 33
        || !bytes.starts_with(PNG_SIGNATURE)
        || &bytes[12..16] != b"IHDR"
    {
        return Err("來源縮圖不是有效的有界 PNG。".to_owned());
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
    if width == 0 || height == 0 || width > 640 || height > 640 {
        return Err("來源縮圖尺寸超過限制。".to_owned());
    }
    Ok((width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_paths_use_project_parent_and_reject_workspace_escape() {
        let base =
            std::env::temp_dir().join(format!("storycut-thumbnail-{}", uuid::Uuid::new_v4()));
        let root = base.join("workspace");
        let nested = root.join("專案");
        fs::create_dir_all(&nested).unwrap();
        let project = nested.join("story.storycut.json");
        fs::write(&project, "{}").unwrap();
        fs::write(nested.join("人物.png"), b"fixture").unwrap();
        fs::write(base.join("outside.png"), b"outside").unwrap();
        assert_eq!(
            source_path(&root, &project, "人物.png").unwrap(),
            fs::canonicalize(nested.join("人物.png")).unwrap()
        );
        assert!(
            source_path(&root, &project, "../../outside.png")
                .unwrap_err()
                .contains("PATH_DENIED")
        );
        assert!(
            source_path(&root, &project, "..")
                .unwrap_err()
                .contains("PATH_DENIED")
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn invalid_or_oversized_thumbnail_output_is_rejected() {
        assert!(dimensions(b"not an image").is_err());
        let mut bytes = vec![0; 33];
        bytes[..8].copy_from_slice(PNG_SIGNATURE);
        bytes[12..16].copy_from_slice(b"IHDR");
        bytes[16..20].copy_from_slice(&640_u32.to_be_bytes());
        bytes[20..24].copy_from_slice(&360_u32.to_be_bytes());
        assert_eq!(dimensions(&bytes).unwrap(), (640, 360));
        bytes[16..20].copy_from_slice(&641_u32.to_be_bytes());
        assert!(dimensions(&bytes).is_err());
        assert!(dimensions(&vec![0; MAX_PNG_BYTES as usize + 1]).is_err());
    }

    #[test]
    fn real_decoder_preserves_display_aspect_and_limits_source_pixels() {
        let base = std::env::temp_dir().join(format!(
            "storycut-thumbnail-decode-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&base).unwrap();
        let anamorphic = base.join("non-square-pixels.mkv");
        let generated = Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=size=720x576:duration=0.04",
                "-vf",
                "setsar=16/15",
                "-frames:v",
                "1",
                "-c:v",
                "ffv1",
                "-threads",
                "1",
            ])
            .arg(&anamorphic)
            .output()
            .unwrap();
        assert!(
            generated.status.success(),
            "{}",
            String::from_utf8_lossy(&generated.stderr)
        );
        assert_eq!(
            dimensions(&decode(&anamorphic, false).unwrap()).unwrap(),
            (640, 480)
        );

        let oversized = base.join("too-many-pixels.png");
        let generated = Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=size=4098x4096",
                "-frames:v",
                "1",
                "-c:v",
                "png",
                "-threads",
                "1",
            ])
            .arg(&oversized)
            .output()
            .unwrap();
        assert!(
            generated.status.success(),
            "{}",
            String::from_utf8_lossy(&generated.stderr)
        );
        let error = decode(&oversized, false).unwrap_err();
        assert!(
            error.contains("maximum allowed pixel count") || error.contains("max pixel count"),
            "{error}"
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn video_thumbnail_skips_black_leader_with_real_ffmpeg() {
        let base = std::env::temp_dir().join(format!("storycut-thumbnail-video-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&base).unwrap();
        let clip = base.join("black-then-red.mp4");
        // 0.5 s black followed by 1.5 s red: a naive first frame is black.
        let generated = Command::new("ffmpeg")
            .args(["-v", "error", "-f", "lavfi", "-i", "color=c=black:size=320x180:rate=24:duration=0.5",
                   "-f", "lavfi", "-i", "color=c=red:size=320x180:rate=24:duration=1.5",
                   "-filter_complex", "[0][1]concat=n=2:v=1:a=0", "-pix_fmt", "yuv420p", "-c:v", "libx264"])
            .arg(&clip)
            .output()
            .unwrap();
        assert!(generated.status.success(), "{}", String::from_utf8_lossy(&generated.stderr));
        let png = decode(&clip, true).unwrap();
        assert_eq!(dimensions(&png).unwrap(), (640, 360));
        let mut raw = Command::new("ffmpeg")
            .args(["-v", "error", "-i", "pipe:0", "-f", "rawvideo", "-pix_fmt", "rgb24", "-vf", "scale=1:1", "pipe:1"])
            .stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        use std::io::Write;
        raw.stdin.take().unwrap().write_all(&png).unwrap();
        let out = raw.wait_with_output().unwrap();
        assert_eq!(out.stdout.len(), 3);
        assert!(out.stdout[0] > 100 && out.stdout[1] < 100, "thumbnail is not the red frame: {:?}", out.stdout);
        fs::remove_dir_all(base).unwrap();
    }
}

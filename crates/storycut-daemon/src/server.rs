#[cfg(windows)]
use std::fs::File;
use std::fs::{self, OpenOptions};
#[cfg(windows)]
use std::io::Write;
use std::path::{Path, PathBuf};

use fs2::FileExt;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::protocol::{DaemonError, DaemonRequest, DaemonResponse, protocol_error_response};
use crate::{MAX_FRAME_BYTES, PROTOCOL_VERSION};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerOutcome {
    Owner,
    AlreadyRunning,
}

#[cfg(windows)]
const PENDING_REQUESTS: usize = 16;
#[cfg(windows)]
const OVERLOAD_HANDLERS: usize = 10;
#[cfg(windows)]
const ACCEPTORS: usize = 4;

/// Calculate the per-user, per-workspace endpoint name after validating that
/// the workspace exists and can be canonicalized.
pub fn endpoint_for_workspace(workspace: &Path) -> Result<String, DaemonError> {
    let workspace = canonical_workspace(workspace)?;
    let user_sid = current_user_sid()?;
    let key = format!(
        "{}\n{}",
        user_sid,
        workspace.to_string_lossy().to_lowercase()
    );
    let digest = Sha256::digest(key.as_bytes());
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(format!(r"\\.\pipe\StoryCut-{hex}"))
}

/// Acquire the exclusive workspace lease before exposing the pipe. Only the
/// current Windows user's SID receives access to that pipe.
pub fn serve_workspace(
    workspace: &Path,
    on_ready: impl FnOnce(&str),
) -> Result<ServerOutcome, DaemonError> {
    #[cfg(not(windows))]
    {
        let _ = (workspace, on_ready);
        Err(DaemonError::UnsupportedPlatform)
    }
    #[cfg(windows)]
    {
        let workspace = canonical_workspace(workspace)?;
        let endpoint = endpoint_for_workspace(&workspace)?;
        let lease = open_lease(&workspace)?;
        if let Err(error) = lease.try_lock_exclusive() {
            if error.kind() == std::io::ErrorKind::WouldBlock
                || error.raw_os_error() == fs2::lock_contended_error().raw_os_error()
            {
                if crate::windows_pipe::probe(&endpoint).is_ok() {
                    return Ok(ServerOutcome::AlreadyRunning);
                }
                return Err(DaemonError::LeaseHeldWithoutServer);
            }
            return Err(DaemonError::Initialization(format!(
                "could not acquire workspace lease: {error}"
            )));
        }

        let (sender, receiver) = std::sync::mpsc::sync_channel::<File>(PENDING_REQUESTS);
        let worker_workspace = workspace.clone();
        std::thread::Builder::new()
            .name("storycut-command-dispatch".to_owned())
            .spawn(move || {
                for pipe in receiver {
                    if let Err(error) = crate::windows_pipe::serve_one(pipe, &worker_workspace) {
                        eprintln!("StoryCut daemon rejected an IPC request: {error}");
                    }
                }
            })
            .map_err(|error| {
                DaemonError::Initialization(format!("could not start command dispatcher: {error}"))
            })?;

        let pipe = crate::windows_pipe::create_server_pipe(&endpoint)?;
        on_ready(&endpoint);
        let overflow_handlers = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        for index in 1..ACCEPTORS {
            let endpoint = endpoint.clone();
            let sender = sender.clone();
            let overflow_handlers = overflow_handlers.clone();
            std::thread::Builder::new()
                .name(format!("storycut-ipc-accept-{index}"))
                .spawn(move || accept_loop(endpoint, None, sender, overflow_handlers))
                .map_err(|error| {
                    DaemonError::Initialization(format!("could not start IPC acceptor: {error}"))
                })?;
        }
        accept_loop(endpoint, Some(pipe), sender, overflow_handlers).map(|()| ServerOutcome::Owner)
    }
}

#[cfg(windows)]
fn accept_loop(
    endpoint: String,
    first_pipe: Option<File>,
    sender: std::sync::mpsc::SyncSender<File>,
    overflow_handlers: std::sync::Arc<std::sync::atomic::AtomicUsize>,
) -> Result<(), DaemonError> {
    let mut pipe = first_pipe;
    loop {
        let server_pipe = match pipe.take() {
            Some(pipe) => pipe,
            None => match crate::windows_pipe::create_server_pipe(&endpoint) {
                Ok(pipe) => pipe,
                Err(error) => {
                    eprintln!("StoryCut daemon could not create another pipe instance: {error}");
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    continue;
                }
            },
        };
        let connected = match crate::windows_pipe::accept_connection(server_pipe) {
            Ok(pipe) => pipe,
            Err(error) => {
                eprintln!("StoryCut daemon failed to accept an IPC connection: {error}");
                continue;
            }
        };
        match sender.try_send(connected) {
            Ok(()) => {}
            Err(std::sync::mpsc::TrySendError::Full(pipe)) => {
                let active = overflow_handlers.fetch_update(
                    std::sync::atomic::Ordering::AcqRel,
                    std::sync::atomic::Ordering::Acquire,
                    |count| (count < OVERLOAD_HANDLERS).then_some(count + 1),
                );
                if active.is_ok() {
                    let counter = overflow_handlers.clone();
                    let overflow = std::thread::Builder::new()
                        .name("storycut-overload-reply".to_owned())
                        .spawn(move || {
                            if let Err(error) = crate::windows_pipe::serve_overloaded(pipe) {
                                eprintln!(
                                    "StoryCut daemon could not send overload response: {error}"
                                );
                            }
                            counter.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
                        });
                    if overflow.is_err() {
                        overflow_handlers.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
                    }
                }
                // Above the bounded queue and reply capacity, close the extra
                // connection immediately so this acceptor remains available.
            }
            Err(std::sync::mpsc::TrySendError::Disconnected(_pipe)) => {
                return Err(DaemonError::Initialization(
                    "command dispatcher stopped".to_owned(),
                ));
            }
        }
    }
}

fn canonical_workspace(workspace: &Path) -> Result<PathBuf, DaemonError> {
    let canonical = fs::canonicalize(workspace)
        .map_err(|error| DaemonError::InvalidWorkspace(error.to_string()))?;
    if !canonical.is_dir() {
        return Err(DaemonError::InvalidWorkspace(
            "workspace must be a directory".to_owned(),
        ));
    }
    Ok(canonical)
}

#[cfg(windows)]
fn open_lease(workspace: &Path) -> Result<File, DaemonError> {
    let local_app_data =
        std::env::var_os("LOCALAPPDATA").ok_or(DaemonError::MissingUserDataDirectory)?;
    let key = format!(
        "{}\n{}",
        current_user_sid()?,
        workspace.to_string_lossy().to_lowercase()
    );
    let digest = Sha256::digest(key.as_bytes());
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    let root = PathBuf::from(local_app_data)
        .join("StoryCut")
        .join("daemon");
    fs::create_dir_all(&root).map_err(|error| DaemonError::Initialization(error.to_string()))?;
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join(format!("{hex}.lock")))
        .map_err(|error| {
            DaemonError::Initialization(format!("could not open workspace lease: {error}"))
        })
}

fn dispatch_request(workspace: &Path, request: DaemonRequest) -> DaemonResponse {
    let request_id = request.request_id.clone();
    if request.protocol_version != PROTOCOL_VERSION {
        return protocol_error_response(
            request_id,
            "PROTOCOL_VERSION_UNSUPPORTED",
            "Unsupported IPC protocol version",
        );
    }
    if request_id.is_empty() || request_id.len() > 128 || request_id.chars().any(char::is_control) {
        return protocol_error_response(
            request_id,
            "INVALID_REQUEST_ID",
            "request_id must be 1 to 128 printable characters",
        );
    }

    let (tool, args) = match request.method.as_str() {
        "capabilities"
            if request
                .params
                .as_object()
                .is_some_and(|object| object.is_empty()) =>
        {
            ("storycut_capabilities", json!({}))
        }
        "capabilities" => {
            return protocol_error_response(
                request_id,
                "INVALID_ARGUMENT",
                "capabilities params must be an empty object",
            );
        }
        "project_get" => ("storycut_project_get", request.params),
        "tool_call" => {
            let Some(object) = request.params.as_object() else {
                return protocol_error_response(
                    request_id,
                    "INVALID_ARGUMENT",
                    "tool_call params must be an object",
                );
            };
            if object.len() != 2 || !object.contains_key("tool") || !object.contains_key("args") {
                return protocol_error_response(
                    request_id,
                    "INVALID_ARGUMENT",
                    "tool_call params require exactly tool and args",
                );
            }
            let Some(tool) = object.get("tool").and_then(Value::as_str) else {
                return protocol_error_response(
                    request_id,
                    "INVALID_ARGUMENT",
                    "tool_call params require a tool string",
                );
            };
            let Some(args) = object.get("args").cloned() else {
                return protocol_error_response(
                    request_id,
                    "INVALID_ARGUMENT",
                    "tool_call params require args",
                );
            };
            (tool, args)
        }
        _ => {
            return protocol_error_response(
                request_id,
                "UNKNOWN_METHOD",
                "Supported methods are capabilities, project_get, and tool_call",
            );
        }
    };

    if !storycut_command::supported_tools()
        .iter()
        .any(|supported| supported == tool)
    {
        return protocol_error_response(
            request_id,
            "UNSUPPORTED_FEATURE",
            "The requested command is not currently implemented",
        );
    }

    match storycut_command::dispatch(workspace, tool, args) {
        Ok(mut result) => {
            if let Some(object) = result.as_object_mut() {
                object.insert("request_id".to_owned(), Value::String(request_id.clone()));
                if tool == "storycut_capabilities"
                    && let Some(data) = object.get_mut("data").and_then(Value::as_object_mut)
                {
                    data.insert(
                        "daemon_version".to_owned(),
                        Value::String(env!("CARGO_PKG_VERSION").to_owned()),
                    );
                    data.insert("daemon_ipc".to_owned(), Value::Bool(true));
                    data.insert("background_render_jobs".to_owned(), Value::Bool(false));
                    data.insert("job_cancel".to_owned(), Value::Bool(false));
                }
            }
            DaemonResponse {
                protocol_version: PROTOCOL_VERSION,
                request_id,
                ok: true,
                result: Some(result),
                error: None,
            }
        }
        Err(error) => DaemonResponse {
            protocol_version: PROTOCOL_VERSION,
            request_id,
            ok: false,
            result: None,
            error: Some(crate::RemoteError {
                code: error.code,
                message: error.message,
                details: error.details,
            }),
        },
    }
}

pub(crate) fn handle_frame(workspace: &Path, frame: &[u8]) -> Vec<u8> {
    let request_id = request_id_from_frame(frame);
    let response = match serde_json::from_slice::<DaemonRequest>(frame) {
        Ok(request) => dispatch_request(workspace, request),
        Err(error) => protocol_error_response(
            request_id,
            "INVALID_REQUEST",
            &format!("Invalid JSON request: {error}"),
        ),
    };
    encode_response(response)
}

pub(crate) fn request_id_from_frame(frame: &[u8]) -> String {
    serde_json::from_slice::<Value>(frame)
        .ok()
        .and_then(|value| {
            value
                .get("request_id")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .filter(|request_id| valid_request_id(request_id))
        .unwrap_or_else(|| "invalid".to_owned())
}

fn valid_request_id(request_id: &str) -> bool {
    !request_id.is_empty() && request_id.len() <= 128 && !request_id.chars().any(char::is_control)
}

pub(crate) fn encode_response(response: DaemonResponse) -> Vec<u8> {
    struct BoundedWriter {
        bytes: Vec<u8>,
    }
    impl Write for BoundedWriter {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            if self.bytes.len().saturating_add(buffer.len()) > MAX_FRAME_BYTES {
                return Err(std::io::Error::other("response exceeds 8 MiB"));
            }
            self.bytes.extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let request_id = if valid_request_id(&response.request_id) {
        response.request_id.clone()
    } else {
        "invalid".to_owned()
    };
    let mut writer = BoundedWriter { bytes: Vec::new() };
    if serde_json::to_writer(&mut writer, &response).is_err() {
        let fallback = protocol_error_response(
            &request_id,
            "RESPONSE_TOO_LARGE",
            "Response exceeds the 8 MiB IPC frame limit",
        );
        writer.bytes = serde_json::to_vec(&fallback).unwrap_or_else(|_| {
            br#"{"protocol_version":1,"request_id":"invalid","ok":false,"error":{"code":"RESPONSE_TOO_LARGE","message":"Response exceeds the 8 MiB IPC frame limit","details":null}}"#.to_vec()
        });
    }
    writer.bytes.push(b'\n');
    writer.bytes
}

pub(crate) fn read_frame<R: std::io::Read>(reader: &mut R) -> Result<Vec<u8>, DaemonError> {
    let mut frame = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let count = reader
            .read(&mut chunk)
            .map_err(|error| DaemonError::Ipc(error.to_string()))?;
        if count == 0 {
            return Err(DaemonError::Protocol(
                "connection closed before a complete newline-framed request".to_owned(),
            ));
        }
        let bytes = &chunk[..count];
        if let Some(newline) = bytes.iter().position(|byte| *byte == b'\n') {
            if frame.len() + newline > MAX_FRAME_BYTES {
                return Err(DaemonError::Protocol(
                    "request frame exceeds 8 MiB".to_owned(),
                ));
            }
            frame.extend_from_slice(&bytes[..newline]);
            if bytes[newline + 1..]
                .iter()
                .any(|byte| !byte.is_ascii_whitespace())
            {
                return Err(DaemonError::Protocol(
                    "only one request frame is allowed per connection".to_owned(),
                ));
            }
            return Ok(frame);
        }
        if frame.len() + count > MAX_FRAME_BYTES {
            return Err(DaemonError::Protocol(
                "request frame exceeds 8 MiB".to_owned(),
            ));
        }
        frame.extend_from_slice(bytes);
    }
}

#[cfg(windows)]
fn current_user_sid() -> Result<String, DaemonError> {
    crate::windows_pipe::current_user_sid()
}

#[cfg(not(windows))]
fn current_user_sid() -> Result<String, DaemonError> {
    Err(DaemonError::UnsupportedPlatform)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn line_framing_is_bounded_and_rejects_multiple_requests() {
        assert_eq!(
            read_frame(&mut Cursor::new(b"{}\n" as &[u8])).unwrap(),
            b"{}"
        );
        assert!(read_frame(&mut Cursor::new(b"{}\n{}\n" as &[u8])).is_err());
        let too_large = vec![b'x'; MAX_FRAME_BYTES + 1];
        assert!(read_frame(&mut Cursor::new(too_large)).is_err());
    }

    #[test]
    fn malformed_request_gets_protocol_response_with_request_id_when_available() {
        let dir = tempfile::tempdir().unwrap();
        let response: DaemonResponse = serde_json::from_slice(&handle_frame(
            dir.path(),
            br#"{"request_id":"abc","method":"bad"}"#,
        ))
        .unwrap();
        assert_eq!(response.request_id, "abc");
        assert_eq!(response.error.unwrap().code, "INVALID_REQUEST");
    }

    #[test]
    fn unknown_methods_are_explicitly_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let response: DaemonResponse = serde_json::from_slice(&handle_frame(dir.path(), br#"{"protocol_version":1,"request_id":"abc","method":"render_background","params":{}}"#)).unwrap();
        assert_eq!(response.error.unwrap().code, "UNKNOWN_METHOD");
    }

    #[test]
    fn wrapper_methods_reject_unknown_or_malformed_parameters() {
        let dir = tempfile::tempdir().unwrap();
        let invalid = handle_frame(
            dir.path(),
            br#"{"protocol_version":1,"request_id":"abc","method":"capabilities","params":{"ignored":true}}"#,
        );
        let response: DaemonResponse = serde_json::from_slice(&invalid).unwrap();
        assert_eq!(response.error.unwrap().code, "INVALID_ARGUMENT");

        let invalid = handle_frame(
            dir.path(),
            br#"{"protocol_version":1,"request_id":"abc","method":"tool_call","params":{"tool":"storycut_project_get","args":{},"ignored":true}}"#,
        );
        let response: DaemonResponse = serde_json::from_slice(&invalid).unwrap();
        assert_eq!(response.error.unwrap().code, "INVALID_ARGUMENT");
    }

    #[test]
    fn oversized_success_response_is_replaced_with_a_bounded_error() {
        let response = DaemonResponse {
            protocol_version: PROTOCOL_VERSION,
            request_id: "large-response".to_owned(),
            ok: true,
            result: Some(Value::String("x".repeat(MAX_FRAME_BYTES + 1))),
            error: None,
        };
        let frame = encode_response(response);
        assert!(frame.len() <= MAX_FRAME_BYTES + 1);
        assert!(frame.ends_with(b"\n"));
        let response: DaemonResponse = serde_json::from_slice(&frame).unwrap();
        assert_eq!(response.request_id, "large-response");
        assert_eq!(response.error.unwrap().code, "RESPONSE_TOO_LARGE");
    }
}

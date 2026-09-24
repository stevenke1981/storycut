//! MCP JSON-RPC stdio adapter for the shared StoryCut command dispatcher.
//!
//! The adapter advertises only command-layer tools that also exist in the
//! checked-in MCP catalog. Tool schemas remain the source of truth for input
//! validation and advertised annotations.

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use jsonschema::{Validator, validator_for};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, BufRead, Read, Write};
use std::path::{Component, Path, PathBuf};
use thiserror::Error;

const PROTOCOL_VERSION: &str = "2025-11-25";
const MAX_TOOLS_PAGE: usize = 100;
const MAX_JSONRPC_LINE_BYTES: usize = 8 * 1024 * 1024;
const MAX_PREVIEW_PNG_BYTES: u64 = 2 * 1024 * 1024;
const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
const CATALOG: &str = include_str!("../../../contracts/mcp-tools.json");

#[derive(Debug, Error)]
pub enum McpRunError {
    #[error("MCP stdio I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("MCP catalog is invalid: {0}")]
    Catalog(String),
}

struct ToolEntry {
    definition: Value,
    input_validator: Validator,
    output_validator: Validator,
}

/// Serve one JSON-RPC message per input line using injectable streams.
///
/// This is the shared entry point used by protocol tests. CLI applications
/// should generally call [`run_stdio`] and provide the same command-layer
/// `supported_tools` and `dispatch` functions used by their JSON CLI.
pub fn run<R, W, F>(
    mut input: R,
    mut output: W,
    workspace: &Path,
    supported_tools: &[String],
    mut dispatch: F,
) -> Result<(), McpRunError>
where
    R: BufRead,
    W: Write,
    F: FnMut(&Path, &str, Value) -> Result<Value, storycut_command::CommandError>,
{
    let catalog = build_catalog(supported_tools)?;
    let mut state = InitState::NotInitialized;
    let mut request_counter = 0_u64;
    let mut line = Vec::new();

    loop {
        match read_bounded_line(&mut input, &mut line)? {
            BoundedLine::Eof => return Ok(()),
            BoundedLine::TooLong => {
                write_message(
                    &mut output,
                    &rpc_error(
                        Value::Null,
                        -32600,
                        "JSON-RPC message exceeds the size limit",
                    ),
                )?;
                continue;
            }
            BoundedLine::Line => {}
        }
        while matches!(line.last(), Some(b'\n' | b'\r')) {
            line.pop();
        }
        if line.is_empty() {
            continue;
        }

        let message = match serde_json::from_slice::<Value>(&line) {
            Ok(message) => message,
            Err(_) => {
                write_message(&mut output, &rpc_error(Value::Null, -32700, "Parse error"))?;
                continue;
            }
        };

        let Some(object) = message.as_object() else {
            write_message(
                &mut output,
                &rpc_error(Value::Null, -32600, "Invalid Request"),
            )?;
            continue;
        };

        // MCP stdio uses individual JSON-RPC messages rather than JSON-RPC
        // batches. Client responses are ignored because this server never
        // sends server-initiated requests.
        if object.get("method").is_none()
            && (object.contains_key("result") || object.contains_key("error"))
        {
            continue;
        }

        let id = object.get("id").cloned();
        let is_notification = id.is_none();
        let valid_id = id.as_ref().is_none_or(is_valid_id);
        let Some(method) = object.get("method").and_then(Value::as_str) else {
            if !is_notification {
                write_message(
                    &mut output,
                    &rpc_error(id.unwrap_or(Value::Null), -32600, "Invalid Request"),
                )?;
            }
            continue;
        };

        if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") || !valid_id {
            if !is_notification {
                write_message(
                    &mut output,
                    &rpc_error(id.unwrap_or(Value::Null), -32600, "Invalid Request"),
                )?;
            }
            continue;
        }

        let response = match method {
            "initialize" => {
                if is_notification {
                    None
                } else if state != InitState::NotInitialized {
                    Some(rpc_error(
                        id.clone().unwrap(),
                        -32600,
                        "Already initialized",
                    ))
                } else if validate_initialize_params(object.get("params")) {
                    state = InitState::AwaitingInitialized;
                    Some(json!({
                        "jsonrpc": "2.0",
                        "id": id.clone().unwrap(),
                        "result": {
                            "protocolVersion": PROTOCOL_VERSION,
                            "capabilities": { "tools": {} },
                            "serverInfo": {
                                "name": "storycut-mcp",
                                "version": env!("CARGO_PKG_VERSION")
                            }
                        }
                    }))
                } else {
                    Some(rpc_error(
                        id.clone().unwrap(),
                        -32602,
                        "Invalid initialize parameters",
                    ))
                }
            }
            "notifications/initialized" => {
                if !is_notification {
                    Some(rpc_error(id.clone().unwrap(), -32600, "Invalid Request"))
                } else if state == InitState::AwaitingInitialized {
                    state = InitState::Initialized;
                    None
                } else {
                    None
                }
            }
            "ping" => {
                if is_notification {
                    None
                } else if !params_is_object_or_empty(object.get("params")) {
                    Some(rpc_error(
                        id.clone().unwrap(),
                        -32602,
                        "Invalid ping parameters",
                    ))
                } else {
                    Some(json!({ "jsonrpc": "2.0", "id": id.clone().unwrap(), "result": {} }))
                }
            }
            "tools/list" | "tools/call" if state != InitState::Initialized => {
                if is_notification {
                    None
                } else {
                    Some(rpc_error(
                        id.clone().unwrap(),
                        -32002,
                        "Server not initialized",
                    ))
                }
            }
            "tools/list" => {
                if is_notification {
                    None
                } else {
                    match list_tools(&catalog, object.get("params")) {
                        Ok(result) => Some(
                            json!({ "jsonrpc": "2.0", "id": id.clone().unwrap(), "result": result }),
                        ),
                        Err(message) => Some(rpc_error(id.clone().unwrap(), -32602, &message)),
                    }
                }
            }
            "tools/call" => {
                if is_notification {
                    None
                } else {
                    request_counter = request_counter.saturating_add(1);
                    Some(call_tool(
                        id.clone().unwrap(),
                        object.get("params"),
                        &catalog,
                        workspace,
                        &mut dispatch,
                        request_counter,
                    ))
                }
            }
            method if method.starts_with("notifications/") => None,
            _ if is_notification => None,
            _ => Some(rpc_error(id.clone().unwrap(), -32601, "Method not found")),
        };

        if let Some(response) = response {
            write_message(&mut output, &response)?;
        }
    }
}

/// Bind the adapter to the process stdin/stdout and shared command API.
pub fn run_stdio(
    workspace: &Path,
    supported_tools: &[String],
    dispatch: fn(&Path, &str, Value) -> Result<Value, storycut_command::CommandError>,
) -> Result<(), McpRunError> {
    run(
        io::stdin().lock(),
        io::stdout().lock(),
        workspace,
        supported_tools,
        dispatch,
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InitState {
    NotInitialized,
    AwaitingInitialized,
    Initialized,
}

fn build_catalog(supported_tools: &[String]) -> Result<Vec<ToolEntry>, McpRunError> {
    let root: Value =
        serde_json::from_str(CATALOG).map_err(|error| McpRunError::Catalog(error.to_string()))?;
    let definitions = root
        .get("tools")
        .and_then(Value::as_array)
        .ok_or_else(|| McpRunError::Catalog("top-level tools array is missing".into()))?;
    let supported = supported_tools
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let mut entries = Vec::new();

    for definition in definitions {
        let Some(name) = definition.get("name").and_then(Value::as_str) else {
            return Err(McpRunError::Catalog("tool definition has no name".into()));
        };
        if !supported.contains(name) {
            continue;
        }
        let schema = definition
            .get("inputSchema")
            .ok_or_else(|| McpRunError::Catalog(format!("{name} has no inputSchema")))?;
        let input_validator = validator_for(schema).map_err(|error| {
            McpRunError::Catalog(format!("invalid input schema for {name}: {error}"))
        })?;
        let output_schema = definition
            .get("outputSchema")
            .ok_or_else(|| McpRunError::Catalog(format!("{name} has no outputSchema")))?;
        let output_validator = validator_for(output_schema).map_err(|error| {
            McpRunError::Catalog(format!("invalid output schema for {name}: {error}"))
        })?;
        entries.push(ToolEntry {
            definition: definition.clone(),
            input_validator,
            output_validator,
        });
    }

    Ok(entries)
}

fn list_tools(catalog: &[ToolEntry], params: Option<&Value>) -> Result<Value, String> {
    let params = match params {
        None => None,
        Some(value) => Some(
            value
                .as_object()
                .ok_or("tools/list params must be an object")?,
        ),
    };
    let cursor = params.and_then(|map| map.get("cursor"));
    let offset = match cursor {
        None => 0,
        Some(Value::String(cursor)) => cursor
            .parse::<usize>()
            .map_err(|_| "invalid tools/list cursor".to_owned())?,
        Some(_) => return Err("tools/list cursor must be a string".into()),
    };
    if params.is_some_and(|map| map.keys().any(|key| key != "cursor" && key != "_meta")) {
        return Err("tools/list contains an unknown parameter".into());
    }
    if offset > catalog.len() {
        return Err("tools/list cursor is out of range".into());
    }

    let end = offset.saturating_add(MAX_TOOLS_PAGE).min(catalog.len());
    let tools = catalog[offset..end]
        .iter()
        .map(|entry| entry.definition.clone())
        .collect::<Vec<_>>();
    let mut result = json!({ "tools": tools });
    if end < catalog.len() {
        result["nextCursor"] = Value::String(end.to_string());
    }
    Ok(result)
}

fn call_tool<F>(
    id: Value,
    params: Option<&Value>,
    catalog: &[ToolEntry],
    workspace: &Path,
    dispatch: &mut F,
    request_number: u64,
) -> Value
where
    F: FnMut(&Path, &str, Value) -> Result<Value, storycut_command::CommandError>,
{
    let request_id = format!("mcp-{request_number}");
    let parsed = match parse_call_params(params) {
        Ok(parsed) => parsed,
        Err(message) => return rpc_error(id, -32602, &message),
    };
    let Some(tool) = catalog.iter().find(|tool| {
        tool.definition.get("name").and_then(Value::as_str) == Some(parsed.name.as_str())
    }) else {
        return rpc_error(id, -32602, "Unknown tool");
    };

    if let Some(error) = tool.input_validator.iter_errors(&parsed.arguments).next() {
        let envelope = error_envelope(
            &request_id,
            "INVALID_ARGUMENT",
            &format!("Tool arguments do not match the input schema: {error}"),
            json!({"schema_path":error.schema_path().to_string(), "instance_path":error.instance_path().to_string()}),
        );
        return tool_result(id, envelope, true, None);
    }

    let include_preview = parsed.name == "storycut_job_get"
        && parsed
            .arguments
            .get("include_preview")
            .and_then(Value::as_bool)
            == Some(true);
    let envelope = match dispatch(workspace, &parsed.name, parsed.arguments) {
        Ok(envelope) if envelope.get("ok").and_then(Value::as_bool).is_some() => envelope,
        Ok(_) => error_envelope(
            &request_id,
            "INTERNAL_ERROR",
            "Command dispatcher returned an invalid response envelope",
            json!({}),
        ),
        Err(error) => error_envelope(&request_id, &error.code, &error.message, error.details),
    };
    let mut envelope = if tool.output_validator.is_valid(&envelope) {
        envelope
    } else {
        error_envelope(
            &request_id,
            "INTERNAL_ERROR",
            "Command dispatcher returned a response that violates its output schema",
            json!({}),
        )
    };
    let mut image = None;
    if include_preview && envelope.get("ok").and_then(Value::as_bool) == Some(true) {
        match read_preview_image(workspace, &envelope) {
            Ok(image_content) => image = image_content,
            Err(error) => {
                envelope = error_envelope(&request_id, error.code, error.message, error.details)
            }
        }
    }
    if !tool.output_validator.is_valid(&envelope) {
        envelope = error_envelope(
            &request_id,
            "INTERNAL_ERROR",
            "MCP adapter generated a response that violates its output schema",
            json!({}),
        );
    }
    let failed = envelope.get("ok").and_then(Value::as_bool) != Some(true);
    if failed {
        image = None;
    }
    tool_result(id, envelope, failed, image)
}

struct CallParams {
    name: String,
    arguments: Value,
}

fn parse_call_params(params: Option<&Value>) -> Result<CallParams, String> {
    let object = params
        .and_then(Value::as_object)
        .ok_or_else(|| "tools/call params must be an object".to_owned())?;
    if object
        .keys()
        .any(|key| !matches!(key.as_str(), "name" | "arguments" | "_meta"))
    {
        return Err("tools/call contains an unknown parameter".into());
    }
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| "tools/call requires a string name".to_owned())?
        .to_owned();
    let arguments = object
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    Ok(CallParams { name, arguments })
}

fn tool_result(id: Value, envelope: Value, is_error: bool, image: Option<Value>) -> Value {
    let text = serde_json::to_string(&envelope).unwrap_or_else(|_| "{}".into());
    let mut content = vec![json!({"type":"text", "text":text})];
    if let Some(image) = image {
        content.push(image);
    }
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": content,
            "structuredContent": envelope,
            "isError": is_error
        }
    })
}

fn error_envelope(request_id: &str, code: &str, message: &str, details: Value) -> Value {
    json!({
        "api_version": "0.2.0",
        "ok": false,
        "request_id": request_id,
        "project_id": null,
        "revision": null,
        "data": null,
        "error": {
            "code": code,
            "message": message,
            "retryable": false,
            "details": details
        },
        "warnings": []
    })
}

fn validate_initialize_params(params: Option<&Value>) -> bool {
    let Some(params) = params.and_then(Value::as_object) else {
        return false;
    };
    params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .is_some()
        && params.get("capabilities").is_some_and(Value::is_object)
        && params
            .get("clientInfo")
            .and_then(Value::as_object)
            .is_some_and(|info| {
                info.get("name").and_then(Value::as_str).is_some()
                    && info.get("version").and_then(Value::as_str).is_some()
            })
}

fn params_is_object_or_empty(params: Option<&Value>) -> bool {
    params.is_none_or(|params| {
        params
            .as_object()
            .is_some_and(|object| object.keys().all(|key| key == "_meta"))
    })
}

fn is_valid_id(value: &Value) -> bool {
    value.is_string() || value.as_i64().is_some() || value.as_u64().is_some()
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc":"2.0", "id":id, "error":{"code":code,"message":message} })
}

fn write_message(output: &mut impl Write, value: &Value) -> io::Result<()> {
    serde_json::to_writer(&mut *output, value)?;
    output.write_all(b"\n")?;
    output.flush()
}

enum BoundedLine {
    Eof,
    Line,
    TooLong,
}

/// Read one line without ever retaining more than the protocol message limit.
/// Overlong lines are drained through their newline so the next request stays
/// synchronized with the client.
fn read_bounded_line(input: &mut impl BufRead, line: &mut Vec<u8>) -> io::Result<BoundedLine> {
    line.clear();
    let mut too_long = false;

    loop {
        let available = input.fill_buf()?;
        if available.is_empty() {
            return Ok(if too_long {
                BoundedLine::TooLong
            } else if line.is_empty() {
                BoundedLine::Eof
            } else {
                BoundedLine::Line
            });
        }

        let newline = available.iter().position(|byte| *byte == b'\n');
        let message_bytes = newline.unwrap_or(available.len());
        let consume = message_bytes + usize::from(newline.is_some());

        if !too_long {
            if line.len().saturating_add(message_bytes) <= MAX_JSONRPC_LINE_BYTES {
                line.extend_from_slice(&available[..message_bytes]);
            } else {
                too_long = true;
                line.clear();
            }
        }
        input.consume(consume);

        if newline.is_some() {
            return Ok(if too_long {
                BoundedLine::TooLong
            } else {
                BoundedLine::Line
            });
        }
    }
}

struct PreviewReadError {
    code: &'static str,
    message: &'static str,
    details: Value,
}

fn read_preview_image(
    workspace: &Path,
    envelope: &Value,
) -> Result<Option<Value>, PreviewReadError> {
    let Some(job) = envelope.pointer("/data/job") else {
        return Err(preview_error(
            "INTERNAL_ERROR",
            "Preview job response is malformed",
            json!({}),
        ));
    };
    if job.get("kind").and_then(Value::as_str) != Some("preview_frame")
        || job.get("state").and_then(Value::as_str) != Some("succeeded")
    {
        return Ok(None);
    }

    let artifacts = job
        .get("artifacts")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            preview_error(
                "INTERNAL_ERROR",
                "Preview artifacts are malformed",
                json!({}),
            )
        })?;
    let pngs = artifacts
        .iter()
        .filter(|artifact| artifact.get("mime_type").and_then(Value::as_str) == Some("image/png"))
        .collect::<Vec<_>>();
    if pngs.len() != 1 {
        return Err(preview_error(
            "INTERNAL_ERROR",
            "A successful preview must have exactly one PNG artifact",
            json!({"png_artifact_count":pngs.len()}),
        ));
    }
    let artifact = pngs[0];
    let declared_size = artifact
        .get("size_bytes")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            preview_error(
                "INTERNAL_ERROR",
                "Preview artifact size is malformed",
                json!({}),
            )
        })?;
    if declared_size > MAX_PREVIEW_PNG_BYTES {
        return Err(preview_error(
            "INTERNAL_ERROR",
            "Preview PNG exceeds the 2 MiB MCP image limit",
            json!({"size_bytes":declared_size,"max_bytes":MAX_PREVIEW_PNG_BYTES}),
        ));
    }
    let expected_hash = artifact
        .get("sha256")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            preview_error(
                "INTERNAL_ERROR",
                "Preview artifact hash is malformed",
                json!({}),
            )
        })?;
    let relative_path = artifact
        .get("relative_path")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            preview_error(
                "INTERNAL_ERROR",
                "Preview artifact path is malformed",
                json!({}),
            )
        })?;
    let artifact_path = resolve_workspace_artifact(workspace, relative_path)?;
    let file = File::open(artifact_path).map_err(|_| {
        preview_error(
            "PATH_DENIED",
            "Preview artifact cannot be opened within the workspace",
            json!({}),
        )
    })?;
    let actual_size = file.metadata().map_err(|_| {
        preview_error(
            "INTERNAL_ERROR",
            "Preview artifact metadata is unavailable",
            json!({}),
        )
    })?;
    if !actual_size.is_file() {
        return Err(preview_error(
            "PATH_DENIED",
            "Preview artifact is not a regular file",
            json!({}),
        ));
    }
    if actual_size.len() > MAX_PREVIEW_PNG_BYTES {
        return Err(preview_error(
            "INTERNAL_ERROR",
            "Preview PNG exceeds the 2 MiB MCP image limit",
            json!({"size_bytes":actual_size.len(),"max_bytes":MAX_PREVIEW_PNG_BYTES}),
        ));
    }
    if actual_size.len() != declared_size {
        return Err(preview_error(
            "INTERNAL_ERROR",
            "Preview artifact size does not match its metadata",
            json!({"declared_size_bytes":declared_size,"actual_size_bytes":actual_size.len()}),
        ));
    }

    let mut bytes = Vec::with_capacity(actual_size.len() as usize);
    file.take(MAX_PREVIEW_PNG_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| preview_error("INTERNAL_ERROR", "Preview PNG could not be read", json!({})))?;
    if bytes.len() as u64 > MAX_PREVIEW_PNG_BYTES {
        return Err(preview_error(
            "INTERNAL_ERROR",
            "Preview PNG exceeds the 2 MiB MCP image limit",
            json!({"size_bytes":bytes.len(),"max_bytes":MAX_PREVIEW_PNG_BYTES}),
        ));
    }
    if bytes.len() as u64 != declared_size {
        return Err(preview_error(
            "INTERNAL_ERROR",
            "Preview artifact changed while it was being read",
            json!({"declared_size_bytes":declared_size,"read_size_bytes":bytes.len()}),
        ));
    }
    if !bytes.starts_with(PNG_SIGNATURE) {
        return Err(preview_error(
            "INTERNAL_ERROR",
            "Preview artifact is not a PNG image",
            json!({}),
        ));
    }
    let actual_hash = format!("{:x}", Sha256::digest(&bytes));
    if actual_hash != expected_hash {
        return Err(preview_error(
            "INTERNAL_ERROR",
            "Preview PNG hash does not match its artifact metadata",
            json!({}),
        ));
    }

    Ok(Some(json!({
        "type":"image",
        "data":BASE64_STANDARD.encode(bytes),
        "mimeType":"image/png"
    })))
}

fn resolve_workspace_artifact(
    workspace: &Path,
    relative: &str,
) -> Result<PathBuf, PreviewReadError> {
    let root = fs::canonicalize(workspace).map_err(|_| {
        preview_error(
            "PATH_DENIED",
            "Authorized workspace cannot be resolved",
            json!({}),
        )
    })?;
    let relative_path = Path::new(relative);
    if relative_path.as_os_str().is_empty() || relative_path.is_absolute() {
        return Err(preview_error(
            "PATH_DENIED",
            "Preview artifact path must be workspace-relative",
            json!({}),
        ));
    }

    let components = relative_path.components().collect::<Vec<_>>();
    let mut candidate = root.clone();
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(segment) = component else {
            return Err(preview_error(
                "PATH_DENIED",
                "Preview artifact path contains an unsafe component",
                json!({}),
            ));
        };
        candidate.push(segment);
        let metadata = fs::symlink_metadata(&candidate).map_err(|_| {
            preview_error(
                "PATH_DENIED",
                "Preview artifact path is unavailable",
                json!({}),
            )
        })?;
        if metadata.file_type().is_symlink() {
            return Err(preview_error(
                "PATH_DENIED",
                "Preview artifact path contains a symbolic link",
                json!({}),
            ));
        }
        if index + 1 < components.len() && !metadata.is_dir() {
            return Err(preview_error(
                "PATH_DENIED",
                "Preview artifact parent path is not a directory",
                json!({}),
            ));
        }
        if index + 1 == components.len() && !metadata.is_file() {
            return Err(preview_error(
                "PATH_DENIED",
                "Preview artifact is not a regular file",
                json!({}),
            ));
        }
    }
    let canonical = fs::canonicalize(&candidate).map_err(|_| {
        preview_error(
            "PATH_DENIED",
            "Preview artifact path cannot be resolved",
            json!({}),
        )
    })?;
    if !canonical.starts_with(&root) {
        return Err(preview_error(
            "PATH_DENIED",
            "Preview artifact is outside the authorized workspace",
            json!({}),
        ));
    }
    Ok(canonical)
}

fn preview_error(code: &'static str, message: &'static str, details: Value) -> PreviewReadError {
    PreviewReadError {
        code,
        message,
        details,
    }
}

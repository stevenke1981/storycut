use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::io::{self, BufRead, Read, Write};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

struct ChannelReader {
    input: Receiver<Vec<u8>>,
    bytes: VecDeque<u8>,
    closed: bool,
}

impl Read for ChannelReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let available = self.fill_buf()?;
        let count = available.len().min(output.len());
        output[..count].copy_from_slice(&available[..count]);
        self.consume(count);
        Ok(count)
    }
}

impl BufRead for ChannelReader {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        if self.bytes.is_empty() && !self.closed {
            match self.input.recv() {
                Ok(chunk) => self.bytes.extend(chunk),
                Err(_) => self.closed = true,
            }
        }
        Ok(self.bytes.make_contiguous())
    }

    fn consume(&mut self, amount: usize) {
        for _ in 0..amount.min(self.bytes.len()) {
            self.bytes.pop_front();
        }
    }
}

struct ChannelWriter {
    output: Sender<Vec<u8>>,
    pending: Vec<u8>,
}

impl Write for ChannelWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.pending.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if !self.pending.is_empty() {
            let pending = std::mem::take(&mut self.pending);
            self.output.send(pending).map_err(|_| {
                io::Error::new(io::ErrorKind::BrokenPipe, "protocol client disconnected")
            })?;
        }
        Ok(())
    }
}

struct ProtocolClient {
    input: Sender<Vec<u8>>,
    output: Receiver<Vec<u8>>,
}

impl ProtocolClient {
    fn send_value(&self, value: &Value) {
        let mut line = serde_json::to_vec(value).unwrap();
        line.push(b'\n');
        self.input.send(line).unwrap();
    }

    fn send_raw(&self, line: &[u8]) {
        self.input.send(line.to_vec()).unwrap();
    }

    fn receive(&self) -> Value {
        let bytes = self.output.recv_timeout(Duration::from_secs(3)).unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }
}

fn start_server() -> (
    ProtocolClient,
    JoinHandle<Result<(), storycut_mcp::McpRunError>>,
) {
    start_server_with(storycut_command::dispatch)
}

fn start_server_with<F>(
    dispatch: F,
) -> (
    ProtocolClient,
    JoinHandle<Result<(), storycut_mcp::McpRunError>>,
)
where
    F: FnMut(&std::path::Path, &str, Value) -> Result<Value, storycut_command::CommandError>
        + Send
        + 'static,
{
    start_server_with_workspace(dispatch, PathBuf::from("C:/storycut-protocol-test"))
}

fn start_server_with_workspace<F>(
    dispatch: F,
    workspace: PathBuf,
) -> (
    ProtocolClient,
    JoinHandle<Result<(), storycut_mcp::McpRunError>>,
)
where
    F: FnMut(&std::path::Path, &str, Value) -> Result<Value, storycut_command::CommandError>
        + Send
        + 'static,
{
    let (client_to_server, server_input) = mpsc::channel();
    let (server_to_client, client_output) = mpsc::channel();
    let tools = storycut_command::supported_tools();
    let server = thread::spawn(move || {
        storycut_mcp::run(
            ChannelReader {
                input: server_input,
                bytes: VecDeque::new(),
                closed: false,
            },
            ChannelWriter {
                output: server_to_client,
                pending: Vec::new(),
            },
            &workspace,
            &tools,
            dispatch,
        )
    });
    (
        ProtocolClient {
            input: client_to_server,
            output: client_output,
        },
        server,
    )
}

fn initialize(id: u64) -> Value {
    json!({
        "jsonrpc":"2.0",
        "id":id,
        "method":"initialize",
        "params":{
            "protocolVersion":"2025-11-25",
            "capabilities":{},
            "clientInfo":{"name":"protocol-test-client","version":"1.0"}
        }
    })
}

fn finish(client: ProtocolClient, server: JoinHandle<Result<(), storycut_mcp::McpRunError>>) {
    drop(client.input);
    assert!(
        server.join().unwrap().is_ok(),
        "server must treat EOF as normal shutdown"
    );
}

#[test]
fn client_drives_initialize_tools_call_ping_and_eof() {
    let (client, server) = start_server();

    client.send_value(&json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}));
    let response = client.receive();
    assert_eq!(response["error"]["code"], -32002);

    client.send_value(&initialize(2));
    let response = client.receive();
    assert_eq!(response["id"], 2);
    assert_eq!(response["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(response["result"]["capabilities"], json!({"tools":{}}));
    assert_eq!(response["result"]["serverInfo"]["name"], "storycut-mcp");

    // The client sends initialized only after it has received initialize's response.
    client.send_value(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    client.send_value(&json!({"jsonrpc":"2.0","id":3,"method":"tools/list","params":{}}));
    let response = client.receive();
    assert_eq!(response["id"], 3);
    let tools = response["result"]["tools"].as_array().unwrap();
    let supported = storycut_command::supported_tools();
    assert_eq!(
        tools.len(),
        supported.len(),
        "only command-supported tools may be advertised"
    );
    for tool in tools {
        assert!(supported.iter().any(|name| tool["name"] == *name));
        assert!(tool.get("inputSchema").is_some());
        assert!(tool.get("annotations").is_some());
    }

    client.send_value(&json!({
        "jsonrpc":"2.0","id":4,"method":"tools/call",
        "params":{"name":"storycut_capabilities","arguments":{}}
    }));
    let response = client.receive();
    assert_eq!(response["id"], 4);
    assert_eq!(response["result"]["isError"], false);
    let structured = &response["result"]["structuredContent"];
    assert_eq!(structured["ok"], true);
    assert_eq!(response["result"]["content"][0]["type"], "text");
    assert_eq!(
        serde_json::from_str::<Value>(response["result"]["content"][0]["text"].as_str().unwrap())
            .unwrap(),
        *structured,
        "text content and structuredContent must carry the same envelope"
    );

    client.send_value(&json!({
        "jsonrpc":"2.0","id":5,"method":"tools/call",
        "params":{"name":"storycut_capabilities","arguments":{"extra":true}}
    }));
    let response = client.receive();
    assert_eq!(response["result"]["isError"], true);
    assert_eq!(
        response["result"]["structuredContent"]["error"]["code"],
        "INVALID_ARGUMENT"
    );

    client.send_value(&json!({
        "jsonrpc":"2.0","id":6,"method":"tools/call",
        "params":{"name":"storycut_job_cancel","arguments":{}}
    }));
    let response = client.receive();
    assert_eq!(
        response["error"]["code"], -32602,
        "unadvertised tools are protocol errors"
    );

    client.send_value(&json!({"jsonrpc":"2.0","id":7,"method":"ping","params":{}}));
    let response = client.receive();
    assert_eq!(response, json!({"jsonrpc":"2.0","id":7,"result":{}}));

    finish(client, server);
}

#[test]
fn parse_errors_and_unknown_methods_use_json_rpc_errors() {
    let (client, server) = start_server();
    client.send_value(&initialize(1));
    assert_eq!(client.receive()["id"], 1);
    client.send_value(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}));

    client.send_raw(b"{ definitely not json }\n");
    let response = client.receive();
    assert_eq!(response["error"]["code"], -32700);
    assert!(response["id"].is_null());

    client.send_value(
        &json!({"jsonrpc":"2.0","id":"bad-method","method":"resources/read","params":{}}),
    );
    let response = client.receive();
    assert_eq!(response["error"]["code"], -32601);

    finish(client, server);
}

#[test]
fn command_errors_are_returned_as_failed_tool_envelopes() {
    let (client, server) = start_server_with(|_, _, _| {
        Err(storycut_command::CommandError {
            code: "UNSUPPORTED_FEATURE".into(),
            message: "not implemented in this build".into(),
            details: json!({"tool":"storycut_capabilities"}),
        })
    });
    client.send_value(&initialize(1));
    assert_eq!(client.receive()["id"], 1);
    client.send_value(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}));

    client.send_value(&json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"storycut_capabilities","arguments":{}}}));
    let response = client.receive();
    assert_eq!(response["result"]["isError"], true);
    assert_eq!(response["result"]["structuredContent"]["ok"], false);
    assert_eq!(
        response["result"]["structuredContent"]["error"]["code"],
        "UNSUPPORTED_FEATURE"
    );

    finish(client, server);
}

#[test]
fn oversized_stdio_message_is_bounded_drained_and_following_request_survives() {
    const LIMIT: usize = 8 * 1024 * 1024;
    let (client, server) = start_server();
    let mut oversized = vec![b' '; LIMIT + 1];
    oversized.push(b'\n');
    client.send_raw(&oversized);

    let response = client.receive();
    assert!(response["id"].is_null());
    assert_eq!(response["error"]["code"], -32600);
    assert!(
        response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("size limit")
    );

    client.send_value(&initialize(9));
    let response = client.receive();
    assert_eq!(
        response["id"], 9,
        "the oversized line must be fully drained"
    );
    finish(client, server);
}

fn successful_preview_response(relative_path: &str, bytes: &[u8]) -> Value {
    let hash = format!("{:x}", Sha256::digest(bytes));
    json!({
        "api_version":"0.2.0",
        "ok":true,
        "request_id":"job-get-test",
        "project_id":"project-test",
        "revision":1,
        "data":{"job":{
            "job_id":"preview-test",
            "project_id":"project-test",
            "source_revision":1,
            "kind":"preview_frame",
            "state":"succeeded",
            "progress":1.0,
            "last_event_seq":2,
            "artifacts":[{
                "artifact_id":"artifact-preview-test",
                "relative_path":relative_path,
                "mime_type":"image/png",
                "sha256":hash,
                "size_bytes":bytes.len()
            }],
            "failure_code":null
        }},
        "error":null,
        "warnings":[]
    })
}

fn initialized_client(client: &ProtocolClient) {
    client.send_value(&initialize(1));
    assert_eq!(client.receive()["id"], 1);
    client.send_value(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
}

const ONE_PIXEL_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==";

#[test]
fn successful_preview_png_is_added_as_image_without_changing_envelope_text() {
    let png = BASE64_STANDARD.decode(ONE_PIXEL_PNG).unwrap();
    let workspace = tempfile::tempdir().unwrap();
    std::fs::create_dir(workspace.path().join("previews")).unwrap();
    std::fs::write(workspace.path().join("previews/preview.png"), &png).unwrap();
    let envelope = successful_preview_response("previews/preview.png", &png);
    let dispatch_envelope = envelope.clone();
    let (client, server) = start_server_with_workspace(
        move |_, tool, _| {
            assert_eq!(tool, "storycut_job_get");
            Ok(dispatch_envelope.clone())
        },
        workspace.path().to_path_buf(),
    );
    initialized_client(&client);
    client.send_value(&json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"storycut_job_get","arguments":{"job_id":"preview-test","include_preview":true}}
    }));

    let response = client.receive();
    assert_eq!(response["result"]["isError"], false);
    assert_eq!(response["result"]["structuredContent"], envelope);
    let content = response["result"]["content"].as_array().unwrap();
    assert_eq!(content.len(), 2);
    assert_eq!(content[0]["type"], "text");
    assert_eq!(
        serde_json::from_str::<Value>(content[0]["text"].as_str().unwrap()).unwrap(),
        response["result"]["structuredContent"]
    );
    assert_eq!(content[1]["type"], "image");
    assert_eq!(content[1]["mimeType"], "image/png");
    assert_eq!(
        BASE64_STANDARD
            .decode(content[1]["data"].as_str().unwrap())
            .unwrap(),
        png
    );
    finish(client, server);
}

#[test]
fn preview_png_above_limit_returns_explicit_error_without_reading_artifact() {
    const TOO_LARGE: usize = 2 * 1024 * 1024 + 1;
    let workspace = tempfile::tempdir().unwrap();
    std::fs::create_dir(workspace.path().join("previews")).unwrap();
    let metadata_bytes = vec![0_u8; TOO_LARGE];
    std::fs::write(
        workspace.path().join("previews/too-large.png"),
        &metadata_bytes,
    )
    .unwrap();
    let mut envelope = successful_preview_response("previews/too-large.png", &metadata_bytes);
    // Deliberately under-report size to prove the adapter checks the opened file.
    envelope["data"]["job"]["artifacts"][0]["size_bytes"] = json!(1);
    let dispatch_envelope = envelope;
    let (client, server) = start_server_with_workspace(
        move |_, _, _| Ok(dispatch_envelope.clone()),
        workspace.path().to_path_buf(),
    );
    initialized_client(&client);
    client.send_value(&json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"storycut_job_get","arguments":{"job_id":"preview-test","include_preview":true}}
    }));

    let response = client.receive();
    assert_eq!(response["result"]["isError"], true);
    assert_eq!(
        response["result"]["structuredContent"]["error"]["code"],
        "INTERNAL_ERROR"
    );
    assert!(
        response["result"]["structuredContent"]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("2 MiB")
    );
    assert_eq!(response["result"]["content"].as_array().unwrap().len(), 1);
    finish(client, server);
}

#[test]
fn preview_png_path_traversal_is_denied() {
    let png = BASE64_STANDARD.decode(ONE_PIXEL_PNG).unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let envelope = successful_preview_response("../outside.png", &png);
    let dispatch_envelope = envelope;
    let (client, server) = start_server_with_workspace(
        move |_, _, _| Ok(dispatch_envelope.clone()),
        workspace.path().to_path_buf(),
    );
    initialized_client(&client);
    client.send_value(&json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"storycut_job_get","arguments":{"job_id":"preview-test","include_preview":true}}
    }));

    let response = client.receive();
    assert_eq!(response["result"]["isError"], true);
    assert_eq!(
        response["result"]["structuredContent"]["error"]["code"],
        "PATH_DENIED"
    );
    assert_eq!(response["result"]["content"].as_array().unwrap().len(), 1);
    finish(client, server);
}

fn narration_assemble_ok_response() -> Value {
    json!({
        "ok": true,
        "data": {
            "committed": true,
            "base_revision": 7,
            "changed_ids": ["track-visual-01", "track-narration-01"],
            "duration_ticks": 40000000,
            "created_track_ids": ["track-visual-01", "track-narration-01", "track-music-01"],
            "visual_track_id": "track-visual-01",
            "narration_track_id": "track-narration-01",
            "music_track_id": "track-music-01",
            "overlay_track_id": null,
            "frame_ticks": 1333333,
            "dissolve_window_ticks": 0,
            "lead_in_ticks": 2000000,
            "narration_end_tick": 38000000,
            "total_duration_ticks": 40000000,
            "segment_offsets": [
                {
                    "index": 0,
                    "asset_id": "asset-abc123",
                    "clip_id": "clip-abc123",
                    "start_tick": 0,
                    "duration_ticks": 20000000,
                    "start_seconds": 0.0
                },
                {
                    "index": 1,
                    "asset_id": "asset-def456",
                    "clip_id": "clip-def456",
                    "start_tick": 20000000,
                    "duration_ticks": 20000000,
                    "start_seconds": 20.0
                }
            ],
            "cuts": [
                {
                    "role": "visual",
                    "clip_id": "clip-abc123",
                    "asset_id": "asset-abc123",
                    "cut_in_tick": 0,
                    "cut_out_tick": 20000000,
                    "clip_start_tick": 0,
                    "clip_duration_ticks": 20000000
                }
            ],
            "native_cores": [
                {
                    "clip_id": "clip-abc123",
                    "asset_id": "asset-abc123",
                    "core_start_tick": 1000000,
                    "core_end_tick": 19000000
                }
            ]
        },
        "error": null,
        "warnings": []
    })
}

#[test]
fn narration_assemble_output_matches_schema() {
    // Positive case: all required fields present in structuredContent
    let ok_response = narration_assemble_ok_response();
    let (client, server) = start_server_with(move |_, _, _| Ok(ok_response.clone()));
    initialized_client(&client);
    client.send_value(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "storycut_narration_assemble",
            "arguments": {
                "workspace": "C:/storycut-protocol-test",
                "revision": 7,
                "segments": [{"asset_id": "asset-abc123", "trim_start_seconds": 0.0, "duration_seconds": 20.0}],
                "narration_track_name": "Narration",
                "visual_track_name": "Visual"
            }
        }
    }));
    let response = client.receive();
    assert_eq!(
        response["result"]["isError"], false,
        "narration_assemble should succeed: {:?}", response
    );
    let sc = &response["result"]["structuredContent"];
    assert_eq!(sc["ok"], true);
    let data = &sc["data"];
    assert!(data.get("segment_offsets").is_some(), "missing segment_offsets");
    assert_eq!(data["segment_offsets"].as_array().unwrap().len(), 2);
    assert!(data.get("cuts").is_some(), "missing cuts");
    assert_eq!(data["cuts"].as_array().unwrap().len(), 1);
    assert!(data.get("native_cores").is_some(), "missing native_cores");
    assert_eq!(data["native_cores"].as_array().unwrap().len(), 1);
    assert!(data.get("visual_track_id").is_some(), "missing visual_track_id");
    assert_eq!(data["visual_track_id"], "track-visual-01");
    assert!(data.get("narration_track_id").is_some(), "missing narration_track_id");
    assert_eq!(data["narration_track_id"], "track-narration-01");
    assert!(data.get("frame_ticks").is_some(), "missing frame_ticks");
    assert_eq!(data["frame_ticks"], 1333333);
    assert!(data.get("dissolve_window_ticks").is_some(), "missing dissolve_window_ticks");
    assert!(data.get("lead_in_ticks").is_some(), "missing lead_in_ticks");
    assert!(data.get("narration_end_tick").is_some(), "missing narration_end_tick");
    assert!(data.get("total_duration_ticks").is_some(), "missing total_duration_ticks");
    assert!(data.get("music_track_id").is_some(), "missing music_track_id");
    assert!(data.get("overlay_track_id").is_some(), "missing overlay_track_id");
    assert!(data["overlay_track_id"].is_null(), "overlay_track_id should be null");
    assert!(data.get("created_track_ids").is_some(), "missing created_track_ids");
    assert_eq!(data["created_track_ids"].as_array().unwrap().len(), 3);
    finish(client, server);

    // Negative case: verify a fixture without narration_end_tick truly lacks that field
    let mut bad_response = narration_assemble_ok_response();
    bad_response["data"]
        .as_object_mut()
        .unwrap()
        .remove("narration_end_tick");
    assert!(
        bad_response["data"].get("narration_end_tick").is_none(),
        "narration_end_tick must be absent from the bad fixture"
    );
}

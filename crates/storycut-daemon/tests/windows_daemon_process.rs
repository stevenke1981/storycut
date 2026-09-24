#![cfg(windows)]

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;
use storycut_core::{Canvas, ProjectStore};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, OPEN_EXISTING, ReadFile, WriteFile,
};

const GENERIC_READ_WRITE: u32 = 0x8000_0000 | 0x4000_0000;

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

fn daemon_exe() -> &'static str {
    env!("CARGO_BIN_EXE_storycut-daemon")
}

fn call_daemon(workspace: &Path, method: &str, params: &Value, request_id: &str) -> Value {
    let output = Command::new(daemon_exe())
        .args(["request", "--workspace"])
        .arg(workspace)
        .args(["--method", method, "--request-id", request_id, "--params"])
        .arg(serde_json::to_string(params).unwrap())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .output()
        .expect("request helper should start");
    assert!(
        output.status.success(),
        "request failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("request helper should print one JSON response")
}

#[test]
fn single_owner_client_eof_and_project_round_trip() {
    let parent = tempfile::tempdir().unwrap();
    let workspace = parent.path().join("剪輯 workspace with spaces");
    std::fs::create_dir(&workspace).unwrap();
    let store = ProjectStore::create(
        &workspace,
        "acceptance.storycut.json",
        "Daemon IPC acceptance",
        Canvas::default(),
        48_000,
        "daemon-ipc-fixture",
    )
    .unwrap();
    let project_id = store.snapshot().unwrap().project_id;

    let first = Command::new(daemon_exe())
        .args(["serve", "--workspace"])
        .arg(&workspace)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let mut first = ChildGuard(first);

    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if let Ok(response) = storycut_daemon::request_workspace(
            &workspace,
            "startup-probe",
            "capabilities",
            serde_json::json!({}),
        ) {
            assert!(
                response.ok,
                "capabilities probe should succeed: {response:?}"
            );
            break;
        }
        if let Some(status) = first.0.try_wait().unwrap() {
            panic!("daemon exited before accepting connections: {status}");
        }
        assert!(
            Instant::now() < deadline,
            "daemon did not create its named pipe"
        );
        thread::sleep(Duration::from_millis(80));
    }

    let mut second = Command::new(daemon_exe())
        .args(["serve", "--workspace"])
        .arg(&workspace)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let second_deadline = Instant::now() + Duration::from_secs(5);
    while second.try_wait().unwrap().is_none() && Instant::now() < second_deadline {
        thread::sleep(Duration::from_millis(50));
    }
    if second.try_wait().unwrap().is_none() {
        let _ = second.kill();
        let _ = second.wait();
        panic!("second daemon did not exit after connecting to the existing owner");
    }
    let second_output = second.wait_with_output().unwrap();
    assert!(
        second_output.status.success(),
        "second daemon failed: {}",
        String::from_utf8_lossy(&second_output.stderr)
    );
    assert!(String::from_utf8_lossy(&second_output.stdout).contains("already_running"));

    let capabilities = call_daemon(
        &workspace,
        "capabilities",
        &serde_json::json!({}),
        "acceptance-capabilities",
    );
    assert_eq!(capabilities["request_id"], "acceptance-capabilities");
    assert_eq!(capabilities["ok"], true);
    assert!(
        capabilities["result"]["data"]["implemented_tools"]
            .as_array()
            .is_some()
    );

    let project = call_daemon(
        &workspace,
        "project_get",
        &serde_json::json!({"project_id": project_id}),
        "acceptance-project-get",
    );
    assert_eq!(project["request_id"], "acceptance-project-get");
    assert_eq!(project["ok"], true);
    assert_eq!(
        project["result"]["data"]["project"]["project_id"],
        project_id
    );

    // Each helper process closed its pipe after one response; the owner remains.
    assert!(
        first.0.try_wait().unwrap().is_none(),
        "client EOF must not stop daemon owner"
    );
    let after_eof = storycut_daemon::request_workspace(
        &workspace,
        "eof-probe",
        "capabilities",
        serde_json::json!({}),
    )
    .unwrap();
    assert!(after_eof.ok);

    // Hold the serialized dispatcher with an incomplete frame while several
    // clients connect. The listeners must keep accepting; queued calls should
    // complete after the held client closes, even when that takes >3 seconds.
    let endpoint = storycut_daemon::endpoint_for_workspace(&workspace).unwrap();
    let endpoint_wide: Vec<u16> = endpoint.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: endpoint_wide is NUL-terminated UTF-16, and null security/template
    // handles select the default client settings.
    let slow_client = unsafe {
        CreateFileW(
            endpoint_wide.as_ptr(),
            GENERIC_READ_WRITE,
            0,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        )
    };
    assert_ne!(slow_client, INVALID_HANDLE_VALUE);
    let mut written = 0u32;
    let partial_frame = b"{\"protocol_version\":1,";
    // SAFETY: the handle was successfully opened above, the byte slice is live
    // for its exact length, and null OVERLAPPED selects synchronous I/O.
    assert_ne!(
        unsafe {
            WriteFile(
                slow_client,
                partial_frame.as_ptr(),
                partial_frame.len() as u32,
                &mut written,
                std::ptr::null_mut(),
            )
        },
        0
    );
    assert_eq!(written, partial_frame.len() as u32);
    thread::sleep(Duration::from_millis(200));

    // Carry the uniquely-owned raw HANDLE as usize because raw pointers are
    // not Send; the worker restores and closes that same handle exactly once.
    let slow_handle = slow_client as usize;
    let slow_request = thread::spawn(move || {
        thread::sleep(Duration::from_millis(3_500));
        let handle = slow_handle as HANDLE;
        let remainder =
            b"\"request_id\":\"slow-client\",\"method\":\"capabilities\",\"params\":{}}\n";
        let mut written = 0u32;
        // SAFETY: this thread exclusively owns the still-open handle and the
        // remainder buffer stays alive for the synchronous write.
        assert_ne!(
            unsafe {
                WriteFile(
                    handle,
                    remainder.as_ptr(),
                    remainder.len() as u32,
                    &mut written,
                    std::ptr::null_mut(),
                )
            },
            0
        );
        assert_eq!(written, remainder.len() as u32);
        let mut response = Vec::new();
        let mut buffer = [0u8; 4096];
        loop {
            let mut read = 0u32;
            // SAFETY: `handle` remains open, `buffer` is writable for its full
            // length, and null OVERLAPPED selects synchronous pipe I/O.
            assert_ne!(
                unsafe {
                    ReadFile(
                        handle,
                        buffer.as_mut_ptr(),
                        buffer.len() as u32,
                        &mut read,
                        std::ptr::null_mut(),
                    )
                },
                0
            );
            response.extend_from_slice(&buffer[..read as usize]);
            if response.contains(&b'\n') {
                break;
            }
        }
        let response: Value =
            serde_json::from_slice(response.split(|byte| *byte == b'\n').next().unwrap()).unwrap();
        assert_eq!(response["request_id"], "slow-client");
        assert_eq!(response["ok"], true);
        // SAFETY: this thread owns the handle and is its only closer.
        unsafe { CloseHandle(handle) };
    });
    let started = Instant::now();
    let (reply_sender, reply_receiver) = std::sync::mpsc::channel();
    let clients: Vec<_> = (0..18)
        .map(|index| {
            let workspace = workspace.clone();
            let reply_sender = reply_sender.clone();
            thread::spawn(move || {
                let response = storycut_daemon::request_workspace(
                    &workspace,
                    &format!("queued-client-{index}"),
                    "capabilities",
                    serde_json::json!({}),
                );
                reply_sender.send((index, response)).unwrap();
            })
        })
        .collect();
    drop(reply_sender);

    // The active slow call plus 16 queued calls must fill the FIFO. At least
    // one additional call must receive a bounded OVERLOADED response promptly.
    let mut saw_overloaded = false;
    let mut completed = Vec::new();
    while !saw_overloaded {
        let reply = reply_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("a full bounded queue should answer excess calls promptly");
        if matches!(reply.1, Err(storycut_daemon::DaemonError::Overloaded)) {
            saw_overloaded = true;
        }
        completed.push(reply);
    }
    assert!(saw_overloaded);

    let crowded_client = storycut_client::DaemonClient::connect_or_start(
        &workspace,
        Path::new("should-not-start-a-second-daemon.exe"),
        Duration::from_millis(500),
    );
    assert!(matches!(
        crowded_client,
        Err(storycut_daemon::DaemonError::Overloaded)
    ));

    // Losing the lease while the owner is overloaded still identifies the
    // existing daemon as the sole owner instead of reporting a dead endpoint.
    let mut contended_owner = Command::new(daemon_exe())
        .args(["serve", "--workspace"])
        .arg(&workspace)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let owner_deadline = Instant::now() + Duration::from_secs(2);
    while contended_owner.try_wait().unwrap().is_none() && Instant::now() < owner_deadline {
        thread::sleep(Duration::from_millis(25));
    }
    if contended_owner.try_wait().unwrap().is_none() {
        let _ = contended_owner.kill();
        let _ = contended_owner.wait();
        panic!("owner probe did not recognize the overloaded daemon");
    }
    let owner_output = contended_owner.wait_with_output().unwrap();
    assert!(owner_output.status.success());
    assert!(String::from_utf8_lossy(&owner_output.stdout).contains("already_running"));

    while completed.len() < 18 {
        completed.push(
            reply_receiver
                .recv_timeout(Duration::from_secs(10))
                .expect("queued calls should finish after the active request completes"),
        );
    }
    for client in clients {
        client.join().unwrap();
    }
    let mut succeeded = 0usize;
    let mut overloaded = 0usize;
    for (index, response) in completed {
        match response {
            Ok(response) => {
                assert!(response.ok, "queued request {index} failed: {response:?}");
                succeeded += 1;
            }
            Err(storycut_daemon::DaemonError::Overloaded) => overloaded += 1,
            Err(error) => panic!("queued request {index} returned {error}"),
        }
    }
    assert_eq!(succeeded + overloaded, 18);
    assert!(overloaded >= 1);
    let connected_client = storycut_client::DaemonClient::connect_or_start(
        &workspace,
        Path::new("unused-when-an-owner-is-live.exe"),
        Duration::from_secs(1),
    )
    .unwrap();
    assert_eq!(
        connected_client.pipe_wait_timeout(),
        storycut_daemon::DEFAULT_PIPE_WAIT_TIMEOUT
    );
    assert!(connected_client.capabilities().unwrap().ok);
    let elapsed = started.elapsed();
    slow_request.join().unwrap();
    assert!(elapsed >= Duration::from_secs(3));
    assert!(first.0.try_wait().unwrap().is_none());
}

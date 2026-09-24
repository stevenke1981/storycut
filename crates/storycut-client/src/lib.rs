//! Client library for the local StoryCut daemon.
//!
//! Timeout values bound waiting for a free named-pipe instance. Once a
//! request connects, request writing and synchronous daemon dispatch/read have
//! no deadline in this version.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use storycut_daemon::{
    DEFAULT_PIPE_WAIT_TIMEOUT, DaemonError, DaemonResponse, endpoint_for_workspace,
};

pub use storycut_daemon::{DaemonRequest, RemoteError};

#[derive(Debug, Clone)]
pub struct DaemonClient {
    workspace: PathBuf,
    pipe_wait_timeout: Duration,
}

impl DaemonClient {
    pub fn connect(workspace: &Path) -> Result<Self, DaemonError> {
        Self::connect_with_timeout(workspace, DEFAULT_PIPE_WAIT_TIMEOUT)
    }

    /// Connect while bounding the wait for a free pipe instance.
    ///
    /// This does not bound the response read after the pipe connects.
    pub fn connect_with_timeout(
        workspace: &Path,
        pipe_wait_timeout: Duration,
    ) -> Result<Self, DaemonError> {
        Self::connect_with_deadlines(workspace, pipe_wait_timeout, pipe_wait_timeout)
    }

    fn connect_with_deadlines(
        workspace: &Path,
        attempt_timeout: Duration,
        operational_timeout: Duration,
    ) -> Result<Self, DaemonError> {
        let workspace = std::fs::canonicalize(workspace)
            .map_err(|error| DaemonError::InvalidWorkspace(error.to_string()))?;
        endpoint_for_workspace(&workspace)?;
        let response = storycut_daemon::request_workspace_with_timeout(
            &workspace,
            &uuid_v4(),
            "capabilities",
            json!({}),
            attempt_timeout,
        )?;
        if !response.ok {
            return Err(DaemonError::Endpoint(
                response
                    .error
                    .map(|error| error.message)
                    .unwrap_or_else(|| "daemon probe failed".to_owned()),
            ));
        }
        Ok(Self {
            workspace,
            pipe_wait_timeout: operational_timeout,
        })
    }

    pub fn capabilities(&self) -> Result<DaemonResponse, DaemonError> {
        self.request("capabilities", json!({}))
    }

    pub fn project_get(&self, project_id: &str) -> Result<DaemonResponse, DaemonError> {
        self.request("project_get", json!({"project_id": project_id}))
    }

    pub fn tool_call(&self, tool: &str, args: Value) -> Result<DaemonResponse, DaemonError> {
        self.request("tool_call", json!({"tool": tool, "args": args}))
    }

    pub fn request(&self, method: &str, params: Value) -> Result<DaemonResponse, DaemonError> {
        let request_id = uuid_v4();
        storycut_daemon::request_workspace_with_timeout(
            &self.workspace,
            &request_id,
            method,
            params,
            self.pipe_wait_timeout,
        )
    }

    /// Get the per-request wait limit for a free named-pipe instance.
    ///
    /// This does not bound the response read after a request connects.
    pub fn pipe_wait_timeout(&self) -> Duration {
        self.pipe_wait_timeout
    }

    /// Connect to an existing owner or start the provided daemon executable.
    /// The daemon process is detached from the adapter and remains its own
    /// workspace owner after this function's caller exits. `timeout` is the
    /// total startup/probe budget for pipe-slot waits and retry delays; a
    /// request accepted by the daemon can still outlive it because synchronous
    /// post-connect reads do not have a deadline yet.
    pub fn connect_or_start(
        workspace: &Path,
        daemon_executable: &Path,
        timeout: Duration,
    ) -> Result<Self, DaemonError> {
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or_else(|| DaemonError::Protocol("startup timeout is too large".to_owned()))?;
        let probe_timeout = timeout.min(Duration::from_millis(250));
        let mut owner_was_overloaded = false;
        match Self::connect_with_deadlines(workspace, probe_timeout, DEFAULT_PIPE_WAIT_TIMEOUT) {
            Ok(client) => return Ok(client),
            Err(DaemonError::Overloaded) => owner_was_overloaded = true,
            Err(_) => {}
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            if !owner_was_overloaded {
                std::process::Command::new(daemon_executable)
                    .arg("serve")
                    .arg("--workspace")
                    .arg(workspace)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .creation_flags(CREATE_NO_WINDOW)
                    .spawn()
                    .map_err(|error| {
                        DaemonError::Initialization(format!("could not start daemon: {error}"))
                    })?;
            }
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    break;
                }
                let probe_timeout = remaining.min(Duration::from_millis(250));
                match Self::connect_with_deadlines(
                    workspace,
                    probe_timeout,
                    DEFAULT_PIPE_WAIT_TIMEOUT,
                ) {
                    Ok(client) => return Ok(client),
                    Err(DaemonError::Overloaded) => owner_was_overloaded = true,
                    Err(_) => {}
                }
                std::thread::sleep(remaining.min(Duration::from_millis(50)));
            }
            if owner_was_overloaded {
                Err(DaemonError::Overloaded)
            } else {
                Err(DaemonError::Timeout)
            }
        }
        #[cfg(not(windows))]
        {
            let _ = (daemon_executable, timeout, owner_was_overloaded);
            Err(DaemonError::UnsupportedPlatform)
        }
    }
}

fn uuid_v4() -> String {
    uuid::Uuid::new_v4().to_string()
}

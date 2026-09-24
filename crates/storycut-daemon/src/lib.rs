//! Workspace-scoped StoryCut command owner and private local IPC server.
//!
//! The first version is deliberately a synchronous command server. It does not
//! claim to own background render jobs or support cancellation.

mod protocol;
mod server;
pub(crate) mod windows_pipe;

pub use protocol::{DaemonError, DaemonRequest, DaemonResponse, RemoteError};
pub use server::{ServerOutcome, endpoint_for_workspace, serve_workspace};
pub use std::time::Duration;

pub const DEFAULT_PIPE_WAIT_TIMEOUT: Duration = Duration::from_secs(60);

/// Send one bounded JSON request to the workspace's current-user daemon.
pub fn request_workspace(
    workspace: &std::path::Path,
    request_id: &str,
    method: &str,
    params: serde_json::Value,
) -> Result<DaemonResponse, DaemonError> {
    request_workspace_with_timeout(
        workspace,
        request_id,
        method,
        params,
        DEFAULT_PIPE_WAIT_TIMEOUT,
    )
}

/// Send one request using the supplied deadline for waiting for a free named
/// pipe instance. Once connected, writing and synchronous dispatch/read have no
/// deadline in this version.
pub fn request_workspace_with_timeout(
    workspace: &std::path::Path,
    request_id: &str,
    method: &str,
    params: serde_json::Value,
    timeout: Duration,
) -> Result<DaemonResponse, DaemonError> {
    let endpoint = endpoint_for_workspace(workspace)?;
    windows_pipe::request(&endpoint, request_id, method, params, timeout)
}

pub const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;
pub const PROTOCOL_VERSION: u32 = 1;

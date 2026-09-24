use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::PROTOCOL_VERSION;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DaemonRequest {
    pub protocol_version: u32,
    pub request_id: String,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DaemonResponse {
    pub protocol_version: u32,
    pub request_id: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RemoteError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteError {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub details: Value,
}

#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    #[error("Windows named-pipe daemon is available only on Windows")]
    UnsupportedPlatform,
    #[error("workspace is not an accessible directory: {0}")]
    InvalidWorkspace(String),
    #[error("LOCALAPPDATA is unavailable; refusing to create an unprotected lease")]
    MissingUserDataDirectory,
    #[error("workspace daemon lease is held but no authorized daemon is responding")]
    LeaseHeldWithoutServer,
    #[error("workspace is already owned by a live daemon")]
    AlreadyRunning,
    #[error("IPC endpoint could not be created: {0}")]
    Endpoint(String),
    #[error("IPC request failed: {0}")]
    Ipc(String),
    #[error("daemon protocol error: {0}")]
    Protocol(String),
    #[error("timed out waiting for a daemon pipe slot")]
    Timeout,
    #[error("daemon is at its bounded request queue limit")]
    Overloaded,
    #[error("daemon initialization failed: {0}")]
    Initialization(String),
}

impl DaemonError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Timeout => "TIMEOUT",
            Self::Overloaded => "OVERLOADED",
            Self::UnsupportedPlatform => "UNSUPPORTED_PLATFORM",
            Self::InvalidWorkspace(_) => "INVALID_WORKSPACE",
            Self::MissingUserDataDirectory => "SECURITY_CONFIGURATION",
            Self::LeaseHeldWithoutServer => "DAEMON_UNAVAILABLE",
            Self::AlreadyRunning => "DAEMON_ALREADY_RUNNING",
            Self::Endpoint(_) => "IPC_UNAVAILABLE",
            Self::Ipc(_) => "IPC_ERROR",
            Self::Protocol(_) => "PROTOCOL_ERROR",
            Self::Initialization(_) => "DAEMON_START_FAILED",
        }
    }
}

pub(crate) fn protocol_error_response(
    request_id: impl Into<String>,
    code: &str,
    message: &str,
) -> DaemonResponse {
    DaemonResponse {
        protocol_version: PROTOCOL_VERSION,
        request_id: request_id.into(),
        ok: false,
        result: None,
        error: Some(RemoteError {
            code: code.to_owned(),
            message: message.to_owned(),
            details: Value::Null,
        }),
    }
}

pub(crate) fn overloaded_response(request_id: impl Into<String>) -> DaemonResponse {
    DaemonResponse {
        protocol_version: PROTOCOL_VERSION,
        request_id: request_id.into(),
        ok: false,
        result: None,
        error: Some(RemoteError {
            code: "OVERLOADED".to_owned(),
            message: "The daemon request queue is full; this request was not dispatched".to_owned(),
            details: Value::Null,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_rejects_unknown_fields_and_response_round_trips_id() {
        let request: DaemonRequest = serde_json::from_value(serde_json::json!({
            "protocol_version": 1,
            "request_id": "r-1",
            "method": "capabilities",
            "params": {}
        }))
        .unwrap();
        assert_eq!(request.request_id, "r-1");
        assert!(
            serde_json::from_value::<DaemonRequest>(serde_json::json!({
                "protocol_version": 1,
                "request_id": "r-1",
                "method": "capabilities",
                "params": {},
                "unexpected": true
            }))
            .is_err()
        );

        let response = DaemonResponse {
            protocol_version: PROTOCOL_VERSION,
            request_id: request.request_id,
            ok: true,
            result: Some(serde_json::json!({"value": 1})),
            error: None,
        };
        let decoded: DaemonResponse =
            serde_json::from_value(serde_json::to_value(response).unwrap()).unwrap();
        assert_eq!(decoded.request_id, "r-1");
    }
}

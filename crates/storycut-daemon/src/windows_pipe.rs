#[cfg(windows)]
mod windows {
    use std::ffi::c_void;
    use std::fs::File;
    use std::io::{Read, Write};
    use std::mem::size_of;
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use std::path::Path;
    use std::ptr::{null, null_mut};
    use std::time::{Duration, Instant};

    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, ERROR_SEM_TIMEOUT, GENERIC_READ,
        GENERIC_WRITE, GetLastError, HANDLE, INVALID_HANDLE_VALUE, LocalFree,
    };
    use windows_sys::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
    };
    use windows_sys::Win32::Security::{
        GetTokenInformation, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE,
        PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    use crate::protocol::{DaemonError, DaemonResponse};
    use crate::server::{handle_frame, read_frame};
    use crate::{MAX_FRAME_BYTES, PROTOCOL_VERSION};

    const PIPE_ACCESS_DUPLEX: u32 = 0x00000003;
    const MAX_INSTANCES: u32 = 32;

    pub struct LocalAllocation(*mut c_void);
    impl Drop for LocalAllocation {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: this wrapper is constructed only from allocations
                // returned by the Windows LocalAlloc-family APIs, and owns
                // each allocation exactly once.
                unsafe {
                    LocalFree(self.0);
                }
            }
        }
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn current_user_sid() -> Result<String, DaemonError> {
        // SAFETY: GetCurrentProcess returns a pseudo-handle valid for this
        // call; OpenProcessToken initializes `token`, which is then closed by
        // the local RAII guard on every exit path.
        unsafe {
            let mut token: HANDLE = null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return Err(DaemonError::Initialization(format!(
                    "OpenProcessToken failed: {}",
                    GetLastError()
                )));
            }
            struct Token(HANDLE);
            impl Drop for Token {
                fn drop(&mut self) {
                    // SAFETY: `Token` uniquely owns the handle returned by
                    // OpenProcessToken and drops it exactly once.
                    unsafe {
                        CloseHandle(self.0);
                    }
                }
            }
            let token = Token(token);
            let mut needed = 0u32;
            // SAFETY: the documented size-query call accepts a null buffer
            // with length zero and writes the required size to `needed`.
            let _ = GetTokenInformation(token.0, TokenUser, null_mut(), 0, &mut needed);
            if needed < size_of::<TOKEN_USER>() as u32 {
                return Err(DaemonError::Initialization(format!(
                    "GetTokenInformation size query failed: {}",
                    GetLastError()
                )));
            }
            let mut buffer = vec![0u8; needed as usize];
            // SAFETY: `buffer` is allocated to the size reported by Windows;
            // the token handle is live and `needed` is writable.
            if GetTokenInformation(
                token.0,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                needed,
                &mut needed,
            ) == 0
            {
                return Err(DaemonError::Initialization(format!(
                    "GetTokenInformation failed: {}",
                    GetLastError()
                )));
            }
            // SAFETY: GetTokenInformation succeeded and returned at least
            // `size_of::<TOKEN_USER>()` bytes. This unaligned read avoids
            // assuming that Vec<u8> is aligned for TOKEN_USER.
            let token_user = std::ptr::read_unaligned(buffer.as_ptr().cast::<TOKEN_USER>());
            // SAFETY: the SID pointer is part of the valid TOKEN_USER record
            // and points into `buffer`, which remains alive for the conversion.
            let user_sid = token_user.User.Sid;
            let mut sid_string = null_mut();
            // SAFETY: `user_sid` points into the successful token query buffer;
            // Windows validates it and initializes the owned output allocation.
            if ConvertSidToStringSidW(user_sid, &mut sid_string) == 0 {
                return Err(DaemonError::Initialization(format!(
                    "ConvertSidToStringSidW failed: {}",
                    GetLastError()
                )));
            }
            let allocation = LocalAllocation(sid_string.cast());
            let mut length = 0usize;
            // SAFETY: the successful conversion returned a NUL-terminated
            // UTF-16 allocation owned by `allocation`.
            while *sid_string.add(length) != 0 {
                length += 1;
            }
            // SAFETY: `length` was measured through the terminator above, so
            // this slice spans exactly the initialized UTF-16 SID characters.
            let sid = String::from_utf16(std::slice::from_raw_parts(sid_string, length))
                .map_err(|error| DaemonError::Initialization(error.to_string()))?;
            drop(allocation);
            Ok(sid)
        }
    }

    fn security_attributes() -> Result<(SECURITY_ATTRIBUTES, LocalAllocation), DaemonError> {
        let sid = current_user_sid()?;
        let sddl = wide(&format!("D:P(A;;GA;;;{sid})"));
        let mut descriptor = null_mut();
        let mut length = 0u32;
        // SAFETY: both inputs are NUL-terminated UTF-16; Windows allocates the
        // returned descriptor, which `LocalAllocation` owns below.
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut descriptor,
                &mut length,
            )
        };
        if ok == 0 || descriptor.is_null() {
            return Err(DaemonError::Initialization(format!(
                "could not build current-user-only pipe ACL: {}",
                // SAFETY: GetLastError has no pointer or handle preconditions.
                unsafe { GetLastError() }
            )));
        }
        let allocation = LocalAllocation(descriptor.cast());
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        Ok((attributes, allocation))
    }

    pub fn create_server_pipe(endpoint: &str) -> Result<File, DaemonError> {
        let (attributes, allocation) = security_attributes()?;
        let endpoint_wide = wide(endpoint);
        // SAFETY: endpoint is NUL-terminated UTF-16, SECURITY_ATTRIBUTES
        // references a live descriptor for this call, and all size values are
        // fixed and within Win32 limits.
        let handle = unsafe {
            CreateNamedPipeW(
                endpoint_wide.as_ptr(),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                MAX_INSTANCES,
                64 * 1024,
                4096,
                5000,
                &attributes,
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(DaemonError::Endpoint(format!(
                "CreateNamedPipeW failed: {}",
                // SAFETY: GetLastError has no pointer or handle preconditions.
                unsafe { GetLastError() }
            )));
        }
        drop(allocation);
        // SAFETY: CreateNamedPipeW returned a valid owned pipe handle, and this
        // is its sole conversion into File ownership.
        Ok(unsafe { File::from_raw_handle(handle.cast()) })
    }

    pub fn accept_connection(pipe: File) -> Result<File, DaemonError> {
        // SAFETY: `pipe` owns a valid server-pipe handle; this endpoint uses
        // PIPE_WAIT, and null OVERLAPPED selects the documented synchronous call.
        let connected = unsafe { ConnectNamedPipe(pipe.as_raw_handle().cast(), null_mut()) };
        // SAFETY: GetLastError reads the error for the ConnectNamedPipe call.
        if connected == 0 && unsafe { GetLastError() } != ERROR_PIPE_CONNECTED {
            return Err(DaemonError::Ipc(format!(
                "ConnectNamedPipe failed: {}",
                // SAFETY: GetLastError has no pointer or handle preconditions.
                unsafe { GetLastError() }
            )));
        }
        Ok(pipe)
    }

    pub fn serve_one(mut pipe: File, workspace: &Path) -> Result<(), DaemonError> {
        let response = match read_frame(&mut pipe) {
            Ok(frame) => handle_frame(workspace, &frame),
            Err(error) => {
                let response = DaemonResponse {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: "invalid".to_owned(),
                    ok: false,
                    result: None,
                    error: Some(crate::RemoteError {
                        code: "INVALID_FRAME".to_owned(),
                        message: error.to_string(),
                        details: serde_json::Value::Null,
                    }),
                };
                crate::server::encode_response(response)
            }
        };
        pipe.write_all(&response)
            .map_err(|error| DaemonError::Ipc(format!("response write failed: {error}")))?;
        // SAFETY: `pipe` still owns the connected server endpoint and remains
        // live until after this disconnect call.
        unsafe {
            DisconnectNamedPipe(pipe.as_raw_handle().cast());
        }
        Ok(())
    }

    pub fn serve_overloaded(mut pipe: File) -> Result<(), DaemonError> {
        let frame = read_frame(&mut pipe)?;
        let response =
            crate::protocol::overloaded_response(crate::server::request_id_from_frame(&frame));
        let bytes = crate::server::encode_response(response);
        pipe.write_all(&bytes).map_err(|error| {
            DaemonError::Ipc(format!("overload response write failed: {error}"))
        })?;
        // SAFETY: `pipe` still owns the connected server endpoint and remains
        // live until after this disconnect call.
        unsafe {
            DisconnectNamedPipe(pipe.as_raw_handle().cast());
        }
        Ok(())
    }

    pub fn probe(endpoint: &str) -> Result<(), DaemonError> {
        match request(
            endpoint,
            "probe",
            "capabilities",
            serde_json::json!({}),
            crate::DEFAULT_PIPE_WAIT_TIMEOUT,
        ) {
            Ok(response) if response.ok => Ok(()),
            // Queue overload still proves that the owner is alive and holds
            // this workspace's endpoint.
            Err(DaemonError::Overloaded) => Ok(()),
            Ok(_) => Err(DaemonError::LeaseHeldWithoutServer),
            Err(error) => Err(error),
        }
    }

    pub fn request(
        endpoint: &str,
        request_id: &str,
        method: &str,
        params: serde_json::Value,
        timeout: Duration,
    ) -> Result<DaemonResponse, DaemonError> {
        let endpoint_wide = wide(endpoint);
        let handle = open_pipe(&endpoint_wide, timeout)?;
        exchange(handle, request_id, method, params)
    }

    fn open_pipe(endpoint_wide: &[u16], timeout: Duration) -> Result<HANDLE, DaemonError> {
        use windows_sys::Win32::System::Pipes::WaitNamedPipeW;
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or_else(|| DaemonError::Protocol("pipe timeout is too large".to_owned()))?;
        loop {
            // SAFETY: the endpoint is NUL-terminated UTF-16; null security and
            // template handles request no custom descriptor/template.
            let handle = unsafe {
                CreateFileW(
                    endpoint_wide.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    0,
                    null(),
                    OPEN_EXISTING,
                    FILE_ATTRIBUTE_NORMAL,
                    null_mut(),
                )
            };
            if handle != INVALID_HANDLE_VALUE {
                return Ok(handle);
            }
            // SAFETY: GetLastError reports the preceding CreateFileW result.
            let error = unsafe { GetLastError() };
            if error != ERROR_PIPE_BUSY {
                return Err(DaemonError::Endpoint(format!(
                    "CreateFileW failed: {error}"
                )));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(DaemonError::Timeout);
            }
            let wait_millis = remaining.as_millis().clamp(1, 1000) as u32;
            // SAFETY: endpoint remains a valid NUL-terminated string for this
            // bounded synchronous wait.
            if unsafe { WaitNamedPipeW(endpoint_wide.as_ptr(), wait_millis) } == 0 {
                // SAFETY: GetLastError reports the preceding wait result.
                let wait_error = unsafe { GetLastError() };
                if wait_error == ERROR_SEM_TIMEOUT || wait_error == ERROR_PIPE_BUSY {
                    continue;
                }
                return Err(DaemonError::Endpoint(format!(
                    "WaitNamedPipeW failed: {wait_error}"
                )));
            }
            // WaitNamedPipe does not reserve the instance. Retry CreateFileW;
            // another client may win the race for it.
        }
    }

    fn exchange(
        handle: HANDLE,
        request_id: &str,
        method: &str,
        params: serde_json::Value,
    ) -> Result<DaemonResponse, DaemonError> {
        // SAFETY: `handle` is the valid handle returned by CreateFileW in
        // open_pipe, and exchange takes its unique ownership.
        let mut pipe = unsafe { File::from_raw_handle(handle.cast()) };
        let mut bytes = serde_json::to_vec(&serde_json::json!({
            "protocol_version": PROTOCOL_VERSION,
            "request_id": request_id,
            "method": method,
            "params": params,
        }))
        .map_err(|error| DaemonError::Protocol(error.to_string()))?;
        if bytes.len() > MAX_FRAME_BYTES {
            return Err(DaemonError::Protocol(
                "request frame exceeds 8 MiB".to_owned(),
            ));
        }
        bytes.push(b'\n');
        let write_error = pipe.write_all(&bytes).err();
        let mut response = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let count = pipe
                .read(&mut chunk)
                .map_err(|error| DaemonError::Ipc(error.to_string()))?;
            if count == 0 {
                return Err(DaemonError::Ipc("daemon closed before replying".to_owned()));
            }
            if let Some(newline) = chunk[..count].iter().position(|byte| *byte == b'\n') {
                if response.len() + newline > MAX_FRAME_BYTES {
                    return Err(DaemonError::Protocol(
                        "response frame exceeds 8 MiB".to_owned(),
                    ));
                }
                response.extend_from_slice(&chunk[..newline]);
                if chunk[newline + 1..count]
                    .iter()
                    .any(|byte| !byte.is_ascii_whitespace())
                {
                    return Err(DaemonError::Protocol(
                        "daemon returned multiple frames".to_owned(),
                    ));
                }
                break;
            }
            if response.len() + count > MAX_FRAME_BYTES {
                return Err(DaemonError::Protocol(
                    "response frame exceeds 8 MiB".to_owned(),
                ));
            }
            response.extend_from_slice(&chunk[..count]);
        }
        let response: DaemonResponse = serde_json::from_slice(&response)
            .map_err(|error| DaemonError::Protocol(error.to_string()))?;
        if response.protocol_version != PROTOCOL_VERSION || response.request_id != request_id {
            return Err(DaemonError::Protocol(
                "daemon response version or request_id did not match".to_owned(),
            ));
        }
        if response
            .error
            .as_ref()
            .is_some_and(|error| error.code == "OVERLOADED")
        {
            return Err(DaemonError::Overloaded);
        }
        if let Some(error) = write_error {
            return Err(DaemonError::Ipc(error.to_string()));
        }
        Ok(response)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn full_pipe_slot_returns_bounded_timeout() {
            let endpoint = wide(&format!(r"\\.\pipe\StoryCutTest-{}", uuid::Uuid::new_v4()));
            // SAFETY: the endpoint buffer is NUL-terminated and all API
            // arguments are valid for a one-instance synchronous test pipe.
            let server = unsafe {
                CreateNamedPipeW(
                    endpoint.as_ptr(),
                    PIPE_ACCESS_DUPLEX,
                    PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                    1,
                    1024,
                    1024,
                    0,
                    null_mut(),
                )
            };
            assert_ne!(server, INVALID_HANDLE_VALUE);
            // SAFETY: endpoint is live NUL-terminated UTF-16 and null security
            // and template handles request defaults.
            let client = unsafe {
                CreateFileW(
                    endpoint.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    0,
                    null(),
                    OPEN_EXISTING,
                    FILE_ATTRIBUTE_NORMAL,
                    null_mut(),
                )
            };
            assert_ne!(client, INVALID_HANDLE_VALUE);
            // SAFETY: `server` is a valid newly-created server-pipe handle and
            // the test uses synchronous PIPE_WAIT mode.
            let connected = unsafe { ConnectNamedPipe(server, null_mut()) };
            // SAFETY: GetLastError reports the ConnectNamedPipe result.
            assert!(connected != 0 || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED);

            let started = Instant::now();
            let result = open_pipe(&endpoint, Duration::from_millis(150));
            assert!(matches!(result, Err(DaemonError::Timeout)));
            assert!(started.elapsed() >= Duration::from_millis(100));
            assert!(started.elapsed() < Duration::from_secs(2));

            // SAFETY: these handles were successfully created above and are
            // owned by this test; each is released exactly once.
            unsafe {
                CloseHandle(client);
                DisconnectNamedPipe(server);
                CloseHandle(server);
            }
        }
    }
}

#[cfg(windows)]
pub(crate) use windows::{
    accept_connection, create_server_pipe, current_user_sid, probe, request, serve_one,
    serve_overloaded,
};

#[cfg(not(windows))]
pub(crate) fn connect(_endpoint: &str) -> Result<(), crate::protocol::DaemonError> {
    Err(crate::protocol::DaemonError::UnsupportedPlatform)
}
#[cfg(not(windows))]
pub(crate) fn create_server_pipe(
    _endpoint: &str,
) -> Result<std::fs::File, crate::protocol::DaemonError> {
    Err(crate::protocol::DaemonError::UnsupportedPlatform)
}
#[cfg(not(windows))]
pub(crate) fn probe(_endpoint: &str) -> Result<(), crate::protocol::DaemonError> {
    Err(crate::protocol::DaemonError::UnsupportedPlatform)
}
#[cfg(not(windows))]
pub(crate) fn request(
    _endpoint: &str,
    _request_id: &str,
    _method: &str,
    _params: serde_json::Value,
    _timeout: std::time::Duration,
) -> Result<crate::protocol::DaemonResponse, crate::protocol::DaemonError> {
    Err(crate::protocol::DaemonError::UnsupportedPlatform)
}
#[cfg(not(windows))]
pub(crate) fn serve_one(
    _pipe: std::fs::File,
    _workspace: &Path,
) -> Result<(), crate::protocol::DaemonError> {
    Err(crate::protocol::DaemonError::UnsupportedPlatform)
}
#[cfg(not(windows))]
pub(crate) fn accept_connection(
    _pipe: std::fs::File,
) -> Result<std::fs::File, crate::protocol::DaemonError> {
    Err(crate::protocol::DaemonError::UnsupportedPlatform)
}
#[cfg(not(windows))]
pub(crate) fn serve_overloaded(_pipe: std::fs::File) -> Result<(), crate::protocol::DaemonError> {
    Err(crate::protocol::DaemonError::UnsupportedPlatform)
}

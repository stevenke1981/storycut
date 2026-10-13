//! Spawn the workspace owner with only NUL inherited. `Command::spawn` on
//! Windows otherwise inherits unrelated inheritable handles, including the
//! caller's protocol/CLI output pipes, which prevents clients observing EOF.
use std::ffi::OsStr;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_ATTRIBUTE_NORMAL, OPEN_EXISTING};
use windows_sys::Win32::System::Threading::*;

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
struct Attributes(LPPROC_THREAD_ATTRIBUTE_LIST);
impl Drop for Attributes {
    fn drop(&mut self) {
        unsafe {
            DeleteProcThreadAttributeList(self.0);
        }
    }
}

fn wide(value: &OsStr) -> io::Result<Vec<u16>> {
    let mut result: Vec<u16> = value.encode_wide().collect();
    if result.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "process argument contains NUL",
        ));
    }
    result.push(0);
    Ok(result)
}

fn quoted(value: &OsStr) -> io::Result<Vec<u16>> {
    let input = wide(value)?;
    let mut result = vec![b'"' as u16];
    let mut slashes = 0;
    for unit in input[..input.len() - 1].iter().copied() {
        if unit == b'\\' as u16 {
            slashes += 1;
            continue;
        }
        if unit == b'"' as u16 {
            result.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2 + 1));
        } else {
            result.extend(std::iter::repeat_n(b'\\' as u16, slashes));
        }
        slashes = 0;
        result.push(unit);
    }
    result.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2));
    result.push(b'"' as u16);
    Ok(result)
}

pub(crate) fn launch(executable: &Path, workspace: &Path) -> io::Result<()> {
    let application = wide(executable.as_os_str())?;
    let mut command = Vec::new();
    for arg in [
        executable.as_os_str(),
        OsStr::new("serve"),
        OsStr::new("--workspace"),
        workspace.as_os_str(),
    ] {
        if !command.is_empty() {
            command.push(b' ' as u16);
        }
        command.extend(quoted(arg)?);
    }
    command.push(0);
    // All buffers and the inherited NUL handle outlive CreateProcessW. The
    // explicit application path and CRT-quoted arguments never invoke a shell.
    unsafe {
        let security = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: std::ptr::null_mut(),
            bInheritHandle: 1,
        };
        let nul = CreateFileW(
            wide(OsStr::new("NUL"))?.as_ptr(),
            0x8000_0000 | 0x4000_0000,
            3,
            &security,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        );
        if nul == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let nul = Handle(nul);
        let mut size = 0;
        InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut size);
        if size == 0 {
            return Err(io::Error::last_os_error());
        }
        // usize storage provides pointer alignment for the opaque Win32 list.
        let mut storage = vec![0_usize; size.div_ceil(std::mem::size_of::<usize>())];
        let list = storage.as_mut_ptr().cast();
        if InitializeProcThreadAttributeList(list, 1, 0, &mut size) == 0 {
            return Err(io::Error::last_os_error());
        }
        let attributes = Attributes(list);
        let handles = [nul.0];
        if UpdateProcThreadAttribute(
            attributes.0,
            0,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
            handles.as_ptr() as *mut _,
            std::mem::size_of_val(&handles),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let mut startup: STARTUPINFOEXW = std::mem::zeroed();
        startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.StartupInfo.hStdInput = nul.0;
        startup.StartupInfo.hStdOutput = nul.0;
        startup.StartupInfo.hStdError = nul.0;
        startup.lpAttributeList = attributes.0;
        let mut process: PROCESS_INFORMATION = std::mem::zeroed();
        if CreateProcessW(
            application.as_ptr(),
            command.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1,
            CREATE_NO_WINDOW | EXTENDED_STARTUPINFO_PRESENT,
            std::ptr::null(),
            std::ptr::null(),
            &startup.StartupInfo,
            &mut process,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let _process = Handle(process.hProcess);
        let _thread = Handle(process.hThread);
        Ok(())
    }
}

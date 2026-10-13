//! Cooperative cancellation and measured FFmpeg progress for a render worker.
use crate::{RenderError, TIMEBASE};
use std::cell::RefCell;
use std::io::{BufRead, BufReader, Read};
use std::process::{Command, Output, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::Duration;

#[derive(Clone, Default)]
pub struct RenderControl {
    cancelled: Arc<AtomicBool>,
    progress: Arc<AtomicU64>,
    verifying: Arc<AtomicBool>,
}

impl RenderControl {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
    pub fn check(&self) -> Result<(), RenderError> {
        if self.cancelled.load(Ordering::Acquire) {
            Err(RenderError::Cancelled)
        } else {
            Ok(())
        }
    }
    pub fn progress(&self) -> Option<f64> {
        let value = self.progress.load(Ordering::Acquire);
        (value > 0).then(|| (value - 1) as f64 / 1_000_000.0)
    }
    pub fn phase(&self) -> &'static str {
        if self.verifying.load(Ordering::Acquire) {
            "verifying"
        } else if self.progress.load(Ordering::Acquire) > 0 {
            "encoding"
        } else {
            "preparing"
        }
    }
    fn update(&self, microseconds: u64, ticks: u64) {
        let fraction =
            (microseconds as f64 * TIMEBASE as f64 / ticks as f64 / 1_000_000.0).clamp(0.0, 0.95);
        self.progress
            .fetch_max((fraction * 1_000_000.0) as u64 + 1, Ordering::AcqRel);
    }
}

thread_local! { static CURRENT: RefCell<Option<RenderControl>> = const { RefCell::new(None) }; }

pub(crate) fn with_control<T>(control: &RenderControl, work: impl FnOnce() -> T) -> T {
    struct Reset(Option<RenderControl>);
    impl Drop for Reset {
        fn drop(&mut self) {
            CURRENT.with(|slot| *slot.borrow_mut() = self.0.take());
        }
    }
    let _reset = Reset(CURRENT.with(|slot| slot.replace(Some(control.clone()))));
    work()
}

pub(crate) fn check() -> Result<(), RenderError> {
    CURRENT.with(|slot| slot.borrow().as_ref().map_or(Ok(()), RenderControl::check))
}

pub(crate) fn verifying() {
    CURRENT.with(|slot| {
        if let Some(control) = slot.borrow().as_ref() {
            control.verifying.store(true, Ordering::Release);
        }
    });
}

pub(crate) trait ControlledCommand {
    fn controlled_output(&mut self) -> Result<Output, RenderError>;
    fn render_output(&mut self, duration_ticks: u64) -> Result<Output, RenderError>;
    fn controlled_byte_count(&mut self) -> Result<(std::process::ExitStatus, u64), RenderError>;
}

impl ControlledCommand for Command {
    fn controlled_output(&mut self) -> Result<Output, RenderError> {
        Ok(run(self, None, false)?.0)
    }
    fn render_output(&mut self, duration_ticks: u64) -> Result<Output, RenderError> {
        Ok(run(self, Some(duration_ticks), false)?.0)
    }
    fn controlled_byte_count(&mut self) -> Result<(std::process::ExitStatus, u64), RenderError> {
        let (output, bytes) = run(self, None, true)?;
        Ok((output.status, bytes))
    }
}

fn run(
    command: &mut Command,
    duration: Option<u64>,
    count_only: bool,
) -> Result<(Output, u64), RenderError> {
    let control = CURRENT.with(|slot| slot.borrow().clone());
    if control.is_none() && !count_only {
        return Ok((command.output()?, 0));
    }
    let control = control.unwrap_or_default();
    control.check()?;
    if duration.is_some() {
        command.args(["-progress", "pipe:1", "-nostats"]);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = command.spawn()?;
    #[cfg(windows)]
    let _ownership = match ProcessOwnership::attach(&child) {
        Ok(owner) => owner,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error.into());
        }
    };
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let progress_control = control.clone();
    let out = std::thread::spawn(move || -> std::io::Result<(Vec<u8>, u64)> {
        let mut reader = BufReader::new(stdout);
        let mut bytes = Vec::new();
        let mut count = 0_u64;
        if let Some(ticks) = duration {
            let mut line = String::new();
            while reader.read_line(&mut line)? > 0 {
                if let Some(value) = line
                    .trim()
                    .strip_prefix("out_time_us=")
                    .and_then(|v| v.parse().ok())
                {
                    progress_control.update(value, ticks);
                }
                line.clear();
            }
        } else if count_only {
            let mut buffer = [0_u8; 64 * 1024];
            loop {
                let read = reader.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                count = count
                    .checked_add(read as u64)
                    .ok_or_else(|| std::io::Error::other("decoded byte count overflow"))?;
            }
        } else {
            reader.read_to_end(&mut bytes)?;
        }
        Ok((bytes, count))
    });
    let err = std::thread::spawn(move || -> std::io::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        BufReader::new(stderr).read_to_end(&mut bytes)?;
        Ok(bytes)
    });
    let status = loop {
        if control.check().is_err() {
            let _ = child.kill();
            break child.wait();
        }
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(error);
            }
        }
    };
    let (stdout, count) = out
        .join()
        .map_err(|_| std::io::Error::other("stdout reader stopped"))??;
    let stderr = err
        .join()
        .map_err(|_| std::io::Error::other("stderr reader stopped"))??;
    control.check()?;
    Ok((
        Output {
            status: status?,
            stdout,
            stderr,
        },
        count,
    ))
}

// Closing this worker-owned handle also kills its subprocess on daemon exit.
#[cfg(windows)]
struct ProcessOwnership(windows_sys::Win32::Foundation::HANDLE);
#[cfg(windows)]
impl ProcessOwnership {
    fn attach(child: &std::process::Child) -> std::io::Result<Self> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::JobObjects::*;
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if handle.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            let owner = Self(handle);
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const _,
                std::mem::size_of_val(&limits) as u32,
            ) == 0
                || AssignProcessToJobObject(handle, child.as_raw_handle() as _) == 0
            {
                return Err(std::io::Error::last_os_error());
            }
            Ok(owner)
        }
    }
}
#[cfg(windows)]
impl Drop for ProcessOwnership {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

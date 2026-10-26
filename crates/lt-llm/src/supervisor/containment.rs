//! Child-process containment and priority, per platform.

use lt_core::error::Result;
use std::process::{Child, Command};

#[cfg(windows)]
pub(super) fn configure(command: &mut Command, below_normal: bool) {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::System::Threading::{BELOW_NORMAL_PRIORITY_CLASS, CREATE_NO_WINDOW};
    let mut flags = CREATE_NO_WINDOW;
    if below_normal {
        flags |= BELOW_NORMAL_PRIORITY_CLASS;
    }
    command.creation_flags(flags);
}

#[cfg(unix)]
pub(super) fn configure(command: &mut Command, _below_normal: bool) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(not(any(windows, unix)))]
pub(super) fn configure(_: &mut Command, _below_normal: bool) {}

/// Best effort, after the child exists.
#[cfg(unix)]
pub(super) fn lower_priority(child: &Child) {
    // SAFETY: plain syscall on our own child's pid; failure is ignored.
    unsafe {
        libc::setpriority(libc::PRIO_PROCESS, child.id(), 10);
    }
}

#[cfg(not(unix))]
pub(super) fn lower_priority(_: &Child) {}

/// Ends the child and anything it started when this is dropped.
#[cfg(windows)]
pub(super) struct Containment(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl Containment {
    pub(super) fn attach(child: &Child) -> Result<Self> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::JobObjects::*;
        // SAFETY: owned handle, correctly-sized zero-initialized Win32 struct;
        // child handle remains alive throughout assignment.
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if handle.is_null() {
                return Err(std::io::Error::last_os_error().into());
            }
            let job = Self(handle);
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const _,
                std::mem::size_of_val(&limits) as u32,
            ) == 0
                || AssignProcessToJobObject(handle, child.as_raw_handle()) == 0
            {
                return Err(std::io::Error::last_os_error().into());
            }
            Ok(job)
        }
    }

    pub(super) fn terminate(&self) {
        // SAFETY: handle is owned and valid until Drop.
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.0, 1);
        }
    }
}

#[cfg(windows)]
impl Drop for Containment {
    fn drop(&mut self) {
        // SAFETY: this is the unique owned job handle.
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

#[cfg(unix)]
pub(super) struct Containment(i32);

#[cfg(unix)]
impl Containment {
    pub(super) fn attach(child: &Child) -> Result<Self> {
        Ok(Self(child.id() as i32))
    }

    pub(super) fn terminate(&self) {
        // SAFETY: group was created for our child, negative PID targets only it.
        unsafe {
            libc::kill(-self.0, libc::SIGKILL);
        }
    }
}

#[cfg(not(any(windows, unix)))]
pub(super) struct Containment;

#[cfg(not(any(windows, unix)))]
impl Containment {
    pub(super) fn attach(_: &Child) -> Result<Self> {
        Ok(Self)
    }

    pub(super) fn terminate(&self) {}
}

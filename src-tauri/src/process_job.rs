//! Makes sure helper processes Prism starts (for example `llama-server`) never outlive Prism.
//!
//! The problem: if Prism crashes or is ended from Task Manager, its `Drop` code does not run, so
//! a child process it started would keep running in the background and keep using memory/CPU.
//!
//! The fix, using a Windows "job object":
//! 1. Create a job with the `KILL_ON_JOB_CLOSE` rule ("when the last handle to this job closes,
//!    end every process inside it").
//! 2. Put the child process into the job right after starting it.
//! 3. Prism holds the only handle to the job. When Prism exits for *any* reason, Windows closes
//!    that handle, so Windows itself ends the child. Dropping [`OwnedJobGuard`] does the same.
//!
//! Safety rules this module follows:
//! - It only uses the handle of a child that Prism itself spawned (`std::process::Child`). It
//!   never looks up processes by name or PID, so it cannot touch another program.
//! - Handles are wrapped in `OwnedHandle`, so they are always closed exactly once.
//!
//! On other operating systems the type exists but does nothing, so callers need no `cfg`.

#[cfg(windows)]
mod imp {
    use std::{
        io,
        os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::Child,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    /// Ends every process assigned to it when dropped (or when Prism exits in any way).
    pub struct OwnedJobGuard {
        job: OwnedHandle,
    }

    impl OwnedJobGuard {
        /// Creates an unnamed job that ends its processes when its last handle closes.
        pub fn new() -> io::Result<Self> {
            // SAFETY: null attributes and a null name create a private, unnamed job object.
            let raw = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if raw.is_null() {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: `raw` is a fresh, valid handle that nothing else owns; OwnedHandle now
            // closes it exactly once.
            let job = unsafe { OwnedHandle::from_raw_handle(raw) };

            // SAFETY: all-zero is a valid "no limits" value for this plain C struct.
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            // SAFETY: the pointer and size describe `limits`, which lives for the whole call.
            let ok = unsafe {
                SetInformationJobObject(
                    job.as_raw_handle(),
                    JobObjectExtendedLimitInformation,
                    (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Self { job })
        }

        /// Puts a child that Prism spawned into the job. Call it right after `spawn()`.
        /// If this fails, the caller should end the child itself (e.g. `child.kill()`).
        pub fn assign(&self, child: &Child) -> io::Result<()> {
            // SAFETY: both handles are valid for the duration of the call: the job is owned by
            // `self` and the process handle is owned by `child`.
            let ok = unsafe {
                AssignProcessToJobObject(self.job.as_raw_handle(), child.as_raw_handle())
            };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use std::{io, process::Child};

    /// No-op on non-Windows platforms (callers still stop children in their own `Drop`).
    pub struct OwnedJobGuard;

    impl OwnedJobGuard {
        pub fn new() -> io::Result<Self> {
            Ok(Self)
        }
        pub fn assign(&self, _child: &Child) -> io::Result<()> {
            Ok(())
        }
    }
}

pub use imp::OwnedJobGuard;

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::{
        os::windows::process::CommandExt,
        process::{Child, Command, Stdio},
        time::{Duration, Instant},
    };

    /// Starts a harmless, window-less process that would otherwise wait about a minute.
    fn waiting_child() -> Child {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        Command::new("ping.exe")
            .args(["-n", "60", "127.0.0.1"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .expect("ping.exe should start")
    }

    /// Waits up to `limit` for the child to exit; returns whether it did.
    fn exits_within(child: &mut Child, limit: Duration) -> bool {
        let start = Instant::now();
        while start.elapsed() < limit {
            if child.try_wait().expect("try_wait").is_some() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    #[test]
    fn dropping_the_guard_ends_assigned_children() {
        let guard = OwnedJobGuard::new().unwrap();
        let mut first = waiting_child();
        let mut second = waiting_child();
        guard.assign(&first).unwrap();
        guard.assign(&second).unwrap();
        // Assigning the same process twice to the same job is allowed and harmless.
        guard.assign(&first).unwrap();
        assert!(
            !exits_within(&mut first, Duration::from_millis(300)),
            "still running"
        );
        drop(guard);
        assert!(exits_within(&mut first, Duration::from_secs(5)));
        assert!(exits_within(&mut second, Duration::from_secs(5)));
    }

    #[test]
    fn unassigned_children_are_not_affected() {
        let guard = OwnedJobGuard::new().unwrap();
        let mut outside = waiting_child();
        drop(guard);
        assert!(!exits_within(&mut outside, Duration::from_millis(500)));
        // Clean up the process this test started.
        outside.kill().unwrap();
        outside.wait().unwrap();
    }

    #[test]
    fn assigning_an_exited_child_reports_an_error() {
        let guard = OwnedJobGuard::new().unwrap();
        let mut child = waiting_child();
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(guard.assign(&child).is_err());
    }
}

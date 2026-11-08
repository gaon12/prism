//! Retain a kernel exit notification while an owned process is still alive.
//! A later PID lookup could accidentally inspect a different, reused process.
use std::{
    io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    time::{Duration, Instant},
};

pub struct ExitProbe {
    queue: OwnedFd,
    pid: u32,
}

impl ExitProbe {
    /// The caller must still own and keep the process alive during registration.
    /// This helper only observes that process; it never sends a signal.
    pub fn new(pid: u32) -> io::Result<Self> {
        if pid == 0 || pid > i32::MAX as u32 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "Invalid PID"));
        }
        let fd = unsafe { libc::kqueue() };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let queue = unsafe { OwnedFd::from_raw_fd(fd) };
        if unsafe { libc::fcntl(queue.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
            return Err(io::Error::last_os_error());
        }
        let change = libc::kevent {
            ident: pid as usize,
            filter: libc::EVFILT_PROC,
            flags: libc::EV_ADD | libc::EV_ENABLE | libc::EV_ONESHOT,
            fflags: libc::NOTE_EXIT,
            data: 0,
            udata: std::ptr::null_mut(),
        };
        let timeout = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // No event is drained here. The same retained queue is used after the
        // process exits, even when its parent has already reaped it.
        if unsafe {
            libc::kevent(
                queue.as_raw_fd(),
                &change,
                1,
                std::ptr::null_mut(),
                0,
                &timeout,
            )
        } < 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { queue, pid })
    }

    pub fn wait(&self, duration: Duration) -> io::Result<()> {
        let deadline = Instant::now()
            .checked_add(duration)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Invalid exit deadline"))?;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let timeout = libc::timespec {
                tv_sec: remaining.as_secs().try_into().map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidInput, "Invalid exit deadline")
                })?,
                tv_nsec: remaining.subsec_nanos().into(),
            };
            let mut event = std::mem::MaybeUninit::<libc::kevent>::uninit();
            let count = unsafe {
                libc::kevent(
                    self.queue.as_raw_fd(),
                    std::ptr::null(),
                    0,
                    event.as_mut_ptr(),
                    1,
                    &timeout,
                )
            };
            if count < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted && Instant::now() < deadline {
                    continue;
                }
                return Err(error);
            }
            if count == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Owned process has not exited",
                ));
            }
            let event = unsafe { event.assume_init() };
            // Darwin's ABI may pack kevent. Copy fields before referencing them.
            let (ident, filter, flags, fflags, data) = (
                event.ident,
                event.filter,
                event.flags,
                event.fflags,
                event.data,
            );
            if flags & libc::EV_ERROR != 0 {
                return Err(io::Error::from_raw_os_error(data as i32));
            }
            if ident != self.pid as usize
                || filter != libc::EVFILT_PROC
                || fflags & libc::NOTE_EXIT == 0
            {
                return Err(io::Error::other("Unexpected process notification"));
            }
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Child, Command, Stdio};

    struct OwnedChild(Child);
    impl Drop for OwnedChild {
        fn drop(&mut self) {
            if matches!(self.0.try_wait(), Ok(None)) {
                let _ = self.0.kill();
            }
            let _ = self.0.wait();
        }
    }

    #[test]
    fn observes_only_the_registered_owned_child_after_it_is_reaped() {
        let mut child = OwnedChild(
            Command::new("/bin/sleep")
                .arg("30")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let probe = ExitProbe::new(child.0.id()).unwrap();
        assert_eq!(
            probe.wait(Duration::from_millis(5)).unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
        child.0.kill().unwrap();
        child.0.wait().unwrap();
        probe.wait(Duration::from_secs(5)).unwrap();
    }

    #[test]
    fn refuses_non_process_identifiers() {
        for pid in [0, u32::MAX] {
            assert!(
                matches!(ExitProbe::new(pid), Err(error) if error.kind() == io::ErrorKind::InvalidInput)
            );
        }
    }
}

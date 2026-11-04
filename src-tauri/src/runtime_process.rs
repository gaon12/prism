//! Own one embedding server. Windows uses a private Job Object; Unix uses a
//! minimal copy of this executable that watches an app-owned stdin pipe.
//! Never look up or terminate another process by its name or a stored PID.
use std::{
    io,
    path::Path,
    process::{Child, Command, ExitStatus},
};

pub struct OwnedRuntimeProcess {
    child: Child,
    #[cfg(windows)]
    _job: crate::process_job::OwnedJobGuard,
    #[cfg(unix)]
    control: Option<std::process::ChildStdin>,
    #[cfg(unix)]
    server_pid: u32,
}

impl OwnedRuntimeProcess {
    /// `command` is the already verified runtime, with inherited environment,
    /// explicit environment overrides and a working directory. Its stdio is
    /// discarded. `launcher` must implement runtime_supervisor::dispatch before
    /// starting its GUI. The executable override also allows isolated QA.
    pub fn spawn(command: Command, launcher: &Path) -> io::Result<Self> {
        #[cfg(unix)]
        {
            crate::runtime_supervisor::spawn(command, launcher)
        }
        #[cfg(not(unix))]
        {
            let mut command = command;
            let _ = launcher;
            #[cfg(windows)]
            let job = crate::process_job::OwnedJobGuard::new()?;
            let mut child = command.spawn()?;
            #[cfg(windows)]
            if let Err(error) = job.assign(&child) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
            Ok(Self {
                child,
                #[cfg(windows)]
                _job: job,
            })
        }
    }
    pub fn id(&self) -> u32 {
        #[cfg(unix)]
        {
            self.server_pid
        }
        #[cfg(not(unix))]
        {
            self.child.id()
        }
    }
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }
    pub fn stop(&mut self) -> io::Result<ExitStatus> {
        #[cfg(unix)]
        {
            use std::time::{Duration, Instant};
            // Closing the only writer also works after SIGKILL/crash, when this
            // Drop implementation never runs. The supervisor kills and reaps
            // its own Child before exiting; do not kill the supervisor first.
            self.control.take();
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                if let Some(status) = self.child.try_wait()? {
                    return Ok(status);
                }
                if Instant::now() >= deadline {
                    // The previous try_wait returned None. Nobody else owns or
                    // waits on this Child, so its leader PID remains reserved,
                    // even if it exits now. process_group(0) made that same PID
                    // a private group ID. Kill only that group before reaping;
                    // never signal a cached PID after it may have been reused.
                    let group = private_group_signal_id(self.child.id())?;
                    // SAFETY: the unreaped Child owns this group identity, and
                    // a negative PID selects that private group, not all users.
                    let result = unsafe { libc::kill(group, libc::SIGKILL) };
                    if result == -1 {
                        let error = io::Error::last_os_error();
                        if error.raw_os_error() != Some(libc::ESRCH) {
                            return Err(error);
                        }
                    }
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        #[cfg(not(unix))]
        {
            let _ = self.child.kill();
        }
        self.child.wait()
    }
    #[cfg(unix)]
    pub(super) fn supervised(mut child: Child) -> io::Result<Self> {
        let control = child.stdin.take();
        Ok(Self {
            child,
            control,
            server_pid: 0,
        })
    }
    #[cfg(unix)]
    pub(super) fn handshake(&mut self) -> io::Result<()> {
        use std::{io::Read, sync::mpsc, time::Duration};
        let mut stdout = self
            .child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("Missing runtime supervisor response"))?;
        let (sender, receiver) = mpsc::sync_channel(1);
        let reader = std::thread::Builder::new()
            .name("prism-runtime-handshake".into())
            .spawn(move || {
                let mut header = [0u8; 13];
                let result = stdout.read_exact(&mut header).map(|_| header);
                let _ = sender.send(result);
            })?;
        let result = receiver
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Runtime supervisor did not respond",
                )
            })?;
        reader
            .join()
            .map_err(|_| io::Error::other("Runtime supervisor reader failed"))?;
        let header = result?;
        if &header[..9] != b"PRISMSUP1" {
            return Err(io::Error::other("Invalid runtime supervisor response"));
        }
        self.server_pid = u32::from_be_bytes(header[9..].try_into().unwrap());
        if self.server_pid == 0 {
            return Err(io::Error::other("Invalid runtime server identity"));
        }
        Ok(())
    }
}
impl Drop for OwnedRuntimeProcess {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

#[cfg(unix)]
fn private_group_signal_id(leader: u32) -> io::Result<i32> {
    // kill(0, ...) and kill(-1, ...) have broadcast semantics. Neither can ever
    // identify the new private child group we own, even in a PID namespace.
    if leader <= 1 {
        return Err(io::Error::other("Invalid runtime supervisor group"));
    }
    i32::try_from(leader)
        .map(|pid| -pid)
        .map_err(|_| io::Error::other("Invalid runtime supervisor group"))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[test]
    fn fallback_group_ids_cannot_become_broadcast_signals() {
        for invalid in [0, 1, u32::MAX] {
            assert!(private_group_signal_id(invalid).is_err());
        }
        assert_eq!(private_group_signal_id(2).unwrap(), -2);
        assert_eq!(private_group_signal_id(i32::MAX as u32).unwrap(), -i32::MAX);
    }
}

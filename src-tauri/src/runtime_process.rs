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
            // Closing the only writer also works after SIGKILL/crash, when this
            // Drop implementation never runs. The supervisor kills and reaps
            // its own Child before exiting; do not kill the supervisor first.
            self.control.take();
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

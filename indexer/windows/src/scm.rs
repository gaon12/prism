//! Read-only service identity checks. Installation/start/stop are deliberately
//! absent here: connecting from the ordinary app cannot elevate or mutate SCM.
use std::{
    io,
    mem::size_of,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    ptr::{null, NonNull},
};
use windows_sys::Win32::{
    Foundation::*,
    System::{Services::*, Threading::*},
};

pub const SERVICE_NAME: &str = "PrismFileIndex";
pub const DISPLAY_NAME: &str = "Prism filename index";
struct ServiceHandle(NonNull<std::ffi::c_void>);
impl ServiceHandle {
    fn new(raw: SC_HANDLE) -> io::Result<Self> {
        Ok(Self(
            NonNull::new(raw).ok_or_else(io::Error::last_os_error)?,
        ))
    }
    fn raw(&self) -> SC_HANDLE {
        self.0.as_ptr()
    }
}
// SAFETY: SCM handles are owned, thread-independent Windows handles. Sharing
// does not transfer close ownership; the wrapper closes exactly once.
unsafe impl Send for ServiceHandle {}
unsafe impl Sync for ServiceHandle {}
impl Drop for ServiceHandle {
    fn drop(&mut self) {
        unsafe {
            CloseServiceHandle(self.raw());
        }
    }
}

fn service() -> io::Result<(ServiceHandle, ServiceHandle)> {
    // SAFETY: local SCM query access, fixed service name, no mutation rights.
    let manager =
        ServiceHandle::new(unsafe { OpenSCManagerW(null(), null(), SC_MANAGER_CONNECT) })?;
    let name: Vec<_> = SERVICE_NAME.encode_utf16().chain(Some(0)).collect();
    let service = ServiceHandle::new(unsafe {
        OpenServiceW(manager.raw(), name.as_ptr(), SERVICE_QUERY_STATUS)
    })?;
    Ok((manager, service))
}
fn status(handle: &ServiceHandle) -> io::Result<SERVICE_STATUS_PROCESS> {
    let mut status = SERVICE_STATUS_PROCESS::default();
    let mut needed = 0;
    // SAFETY: typed initialized output, query-only owned service handle.
    if unsafe {
        QueryServiceStatusEx(
            handle.raw(),
            SC_STATUS_PROCESS_INFO,
            (&mut status as *mut SERVICE_STATUS_PROCESS).cast(),
            size_of::<SERVICE_STATUS_PROCESS>() as u32,
            &mut needed,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(status)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum State {
    NotInstalled,
    Stopped,
    Running,
}
pub fn state() -> io::Result<State> {
    let (_manager, handle) = match service() {
        Ok(pair) => pair,
        Err(error) if error.raw_os_error() == Some(ERROR_SERVICE_DOES_NOT_EXIST as i32) => {
            return Ok(State::NotInstalled)
        }
        Err(error) => return Err(error),
    };
    let status = status(&handle)?;
    Ok(
        if status.dwCurrentState == SERVICE_RUNNING && status.dwProcessId != 0 {
            State::Running
        } else {
            State::Stopped
        },
    )
}

pub(crate) struct PeerIdentity {
    _manager: ServiceHandle,
    service: ServiceHandle,
    process: OwnedHandle,
    pid: u32,
}
impl PeerIdentity {
    pub fn running() -> io::Result<Self> {
        let (manager, service) = service()?;
        let info = status(&service)?;
        if info.dwCurrentState != SERVICE_RUNNING
            || info.dwServiceType != SERVICE_WIN32_OWN_PROCESS
            || info.dwProcessId == 0
        {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "The filename index service is not running.",
            ));
        }
        // Hold the process object, preventing PID reuse while the pipe is in
        // use. The host grants only query/synchronize access to interactive
        // clients; they gain no memory, termination, token or injection rights.
        let raw = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                0,
                info.dwProcessId,
            )
        };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        let identity = Self {
            _manager: manager,
            service,
            process: unsafe { OwnedHandle::from_raw_handle(raw) },
            pid: info.dwProcessId,
        };
        identity.verify_process()?;
        Ok(identity)
    }
    fn verify_process(&self) -> io::Result<()> {
        let info = status(&self.service)?;
        if info.dwCurrentState != SERVICE_RUNNING
            || info.dwProcessId != self.pid
            || unsafe { WaitForSingleObject(self.process.as_raw_handle(), 0) } != WAIT_TIMEOUT
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "The filename index service identity changed.",
            ));
        }
        Ok(())
    }
    pub fn verify_pipe(&self, pipe: HANDLE) -> io::Result<()> {
        verify_pipe_pid(pipe, self.pid)?;
        self.verify_process()
    }
}

pub(crate) fn verify_pipe_pid(pipe: HANDLE, expected: u32) -> io::Result<()> {
    let mut actual = 0;
    // SAFETY: query only, valid owned connected named-pipe handle.
    if unsafe { windows_sys::Win32::System::Pipes::GetNamedPipeServerProcessId(pipe, &mut actual) }
        == 0
    {
        return Err(io::Error::last_os_error());
    }
    if actual == 0 || actual != expected {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Unexpected filename index pipe server.",
        ));
    }
    Ok(())
}

/// Called only by the service host on its own process. Ordinary clients may
/// query its identity and wait for termination; no other rights are granted.
pub fn allow_client_identity_queries() -> io::Result<()> {
    let descriptor = crate::security::Descriptor::parse(
        "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;OW)(A;;0x101000;;;IU)",
    )?;
    // SAFETY: modify only the current host's DACL, not another process/token.
    if unsafe {
        windows_sys::Win32::Security::SetKernelObjectSecurity(
            GetCurrentProcess(),
            windows_sys::Win32::Security::DACL_SECURITY_INFORMATION,
            descriptor.raw(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

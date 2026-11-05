//! Windows SCM entry point for a dedicated, ordinary Win32 service process.
//! This module never installs or starts a service on the caller's machine.
use crate::{
    host::{self, Backend},
    scm,
};
use std::{
    io,
    ptr::null_mut,
    sync::{
        atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering},
        Arc, Mutex, OnceLock,
    },
};
use windows_sys::Win32::{Foundation::*, System::Services::*};

static BACKEND: OnceLock<Arc<dyn Backend>> = OnceLock::new();
static SHUTDOWN: OnceLock<Arc<AtomicBool>> = OnceLock::new();
static STATUS: AtomicPtr<std::ffi::c_void> = AtomicPtr::new(null_mut());
static CURRENT: AtomicU32 = AtomicU32::new(SERVICE_START_PENDING);
static EXIT: AtomicU32 = AtomicU32::new(0);
static CHECKPOINT: AtomicU32 = AtomicU32::new(1);
static STATUS_LOCK: Mutex<()> = Mutex::new(());

fn report(state: u32, exit: u32) {
    let _guard = STATUS_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let previous = CURRENT.load(Ordering::Acquire);
    if previous == SERVICE_STOPPED
        || (previous == SERVICE_STOP_PENDING
            && state != SERVICE_STOP_PENDING
            && state != SERVICE_STOPPED)
    {
        return;
    }
    CURRENT.store(state, Ordering::Release);
    let handle = STATUS.load(Ordering::Acquire);
    if handle.is_null() {
        return;
    }
    let pending = state == SERVICE_START_PENDING || state == SERVICE_STOP_PENDING;
    let status = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: state,
        dwControlsAccepted: if state == SERVICE_RUNNING {
            SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN
        } else {
            0
        },
        dwWin32ExitCode: exit,
        dwServiceSpecificExitCode: 0,
        dwCheckPoint: if pending {
            CHECKPOINT.fetch_add(1, Ordering::Relaxed)
        } else {
            0
        },
        dwWaitHint: if pending { 10000 } else { 0 },
    };
    // SAFETY: the handle belongs to this service, and status storage remains
    // valid for the synchronous call. Only service-control state is changed.
    unsafe {
        SetServiceStatus(handle, &status);
    }
}
unsafe extern "system" fn control(
    code: u32,
    _event: u32,
    _data: *mut std::ffi::c_void,
    _context: *mut std::ffi::c_void,
) -> u32 {
    match code {
        SERVICE_CONTROL_STOP | SERVICE_CONTROL_SHUTDOWN => {
            if let Some(flag) = SHUTDOWN.get() {
                flag.store(true, Ordering::Release);
            }
            report(SERVICE_STOP_PENDING, 0);
            0
        }
        SERVICE_CONTROL_INTERROGATE => {
            report(CURRENT.load(Ordering::Acquire), 0);
            0
        }
        _ => ERROR_CALL_NOT_IMPLEMENTED,
    }
}
unsafe extern "system" fn main(_argc: u32, _argv: *mut *mut u16) {
    let name: Vec<_> = scm::SERVICE_NAME.encode_utf16().chain(Some(0)).collect();
    // SAFETY: fixed service name, static callback and no borrowed context.
    let handle = unsafe { RegisterServiceCtrlHandlerExW(name.as_ptr(), Some(control), null_mut()) };
    if handle.is_null() {
        EXIT.store(unsafe { GetLastError() }, Ordering::Release);
        return;
    }
    STATUS.store(handle, Ordering::Release);
    report(SERVICE_START_PENDING, 0);
    let result = scm::allow_client_identity_queries().and_then(|_| {
        host::serve_ready(
            BACKEND.get().unwrap().clone(),
            SHUTDOWN.get().unwrap().clone(),
            || report(SERVICE_RUNNING, 0),
        )
    });
    let exit = if result.is_ok() {
        0
    } else {
        result
            .as_ref()
            .err()
            .and_then(io::Error::raw_os_error)
            .map_or(ERROR_GEN_FAILURE, |code| code as u32)
    };
    EXIT.store(exit, Ordering::Release);
    report(SERVICE_STOPPED, exit);
}
pub fn run(backend: Arc<dyn Backend>) -> io::Result<()> {
    if BACKEND.set(backend).is_err() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "The service dispatcher has already run.",
        ));
    }
    let _ = SHUTDOWN.set(Arc::new(AtomicBool::new(false)));
    let mut name: Vec<_> = scm::SERVICE_NAME.encode_utf16().chain(Some(0)).collect();
    let table = [
        SERVICE_TABLE_ENTRYW {
            lpServiceName: name.as_mut_ptr(),
            lpServiceProc: Some(main),
        },
        SERVICE_TABLE_ENTRYW {
            lpServiceName: null_mut(),
            lpServiceProc: None,
        },
    ];
    // SAFETY: both entries and name remain alive throughout the dispatcher;
    // callbacks and state are static for the process lifetime.
    if unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let exit = EXIT.load(Ordering::Acquire);
    if exit != 0 {
        return Err(io::Error::from_raw_os_error(exit as i32));
    }
    Ok(())
}

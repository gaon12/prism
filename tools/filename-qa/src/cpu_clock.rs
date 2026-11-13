//! Raw single-interval thread CPU accounting, alongside unconverted cycles.
use std::io;

pub(crate) struct ThreadUsage {
    user: u64,
    kernel: u64,
    cycles: u64,
    thread: u32,
}

impl ThreadUsage {
    #[cfg(windows)]
    pub(crate) fn read() -> io::Result<Self> {
        use windows_sys::Win32::{
            Foundation::{FILETIME, HANDLE},
            System::Threading::{GetCurrentThread, GetCurrentThreadId, GetThreadTimes},
        };
        // QueryThreadCycleTime's ABI, without expanding the production app's
        // windows-sys features for this standalone diagnostic.
        #[link(name = "kernel32")]
        unsafe extern "system" {
            #[link_name = "QueryThreadCycleTime"]
            fn query_cycles(thread: HANDLE, cycles: *mut u64) -> windows_sys::core::BOOL;
        }
        let blank = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let (mut created, mut exited, mut kernel, mut user) = (blank, blank, blank, blank);
        let mut cycles = 0;
        // SAFETY: current-thread pseudo handles need no closing; all outputs
        // point to initialized locals. Live-thread exit time is not inspected.
        let thread = unsafe {
            if GetThreadTimes(
                GetCurrentThread(),
                &mut created,
                &mut exited,
                &mut kernel,
                &mut user,
            ) == 0
                || query_cycles(GetCurrentThread(), &mut cycles) == 0
            {
                return Err(io::Error::last_os_error());
            }
            GetCurrentThreadId()
        };
        let ticks = |value: FILETIME| {
            (u64::from(value.dwHighDateTime) << 32) | u64::from(value.dwLowDateTime)
        };
        Ok(Self {
            user: ticks(user),
            kernel: ticks(kernel),
            cycles,
            thread,
        })
    }

    #[cfg(not(windows))]
    pub(crate) fn read() -> io::Result<Self> {
        Err(io::ErrorKind::Unsupported.into())
    }

    pub(crate) fn since(self, earlier: Self) -> io::Result<serde_json::Value> {
        let subtract = |end: u64, start: u64| {
            end.checked_sub(start)
                .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidData))
        };
        if self.thread != earlier.thread {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let user = subtract(self.user, earlier.user)?;
        let kernel = subtract(self.kernel, earlier.kernel)?;
        Ok(serde_json::json!({
            "userCpuMs":user as f64/10_000.0,
            "kernelCpuMs":kernel as f64/10_000.0,
            "cpuMs":(user+kernel) as f64/10_000.0,
            "threadCycles":subtract(self.cycles,earlier.cycles)?,
            "cpuClock":"Single-interval GetThreadTimes; raw accounting, coarse for short phases",
            "cycleClock":"QueryThreadCycleTime; cycles are not converted to time"
        }))
    }
}

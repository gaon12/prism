//! Operation-start budgets, never per-row probes or claims of a process RSS cap.
//! Keep the existing single worker when headroom is unknown/exhausted. Additional
//! parallel work gets at most a quarter of available RAM, capped at 8 GiB.
const MIB: u64 = 1024 * 1024;
const MAX_SCRATCH: u64 = 8 * 1024 * MIB;
const CPU_WORKER_BYTES: u64 = 8 * MIB;

#[derive(Clone, Copy, Debug, serde::Serialize)]
pub(crate) struct WorkBudget {
    cpus: usize,
    scratch: u64,
}
impl WorkBudget {
    pub(crate) fn current() -> Self {
        Self::from_available(available_cpus(), available_memory())
    }
    fn from_available(cpus: usize, memory: Option<u64>) -> Self {
        Self {
            cpus: cpus.max(1),
            scratch: (memory.unwrap_or(0) / 4).min(MAX_SCRATCH),
        }
    }
    /// A concurrency estimate for additional working storage, not input/model
    /// permission. Each extractor/runtime retains its existing input bounds.
    pub(crate) fn workers_for(self, bytes_per_worker: u64) -> usize {
        let memory = usize::try_from(self.scratch / bytes_per_worker.max(1)).unwrap_or(usize::MAX);
        self.cpus.min(memory).max(1)
    }
    pub(crate) fn cpu_threads(self) -> usize {
        self.workers_for(CPU_WORKER_BYTES)
    }
    pub(crate) fn inference_threads(self) -> usize {
        // Pinned llama.cpp b11960 ggml/include/ggml.h: GGML_MAX_N_THREADS.
        self.cpu_threads().min(512)
    }
    /// Async readiness is separate from CPU inference/extraction parallelism.
    pub(crate) fn io_threads(self) -> usize {
        self.workers_for(2 * MIB).min(4)
    }
}

pub(crate) fn available_cpus() -> usize {
    let all = std::thread::available_parallelism().map_or(1, usize::from);
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessAffinityMask};
        let (mut process, mut system) = (0usize, 0usize);
        // SAFETY: writable locals and a borrowed pseudo handle.
        if unsafe { GetProcessAffinityMask(GetCurrentProcess(), &mut process, &mut system) } != 0 {
            return cpus_allowed(all, process);
        }
    }
    all
}
#[cfg(any(windows, test))]
pub(crate) fn cpus_allowed(all: usize, mask: usize) -> usize {
    match mask.count_ones() as usize {
        0 => all.max(1),
        allowed => all.max(1).min(allowed),
    }
}

/// Only the calling worker is deprioritized. Never lower the UI process.
pub(crate) fn background_thread() {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{
            GetCurrentThread, SetThreadPriority, SetThreadPriorityBoost,
            THREAD_PRIORITY_BELOW_NORMAL,
        };
        // SAFETY: calling-thread pseudo handle, no owned handle to close.
        unsafe {
            SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
            SetThreadPriorityBoost(GetCurrentThread(), 1);
        }
    }
    #[cfg(target_os = "linux")]
    {
        // Linux's PRIO_PROCESS with who=0 changes the calling task/thread.
        // Do not try to raise a worker already at a lower scheduling priority.
        // SAFETY: no pointers; only the calling thread's scheduling priority.
        unsafe {
            if libc::getpriority(libc::PRIO_PROCESS, 0) < 5 {
                libc::setpriority(libc::PRIO_PROCESS, 0, 5);
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        // Darwin nice(PRIO_PROCESS) would also lower the GUI. Use thread QoS.
        // SAFETY: a valid utility class with the required relative priority 0.
        unsafe {
            libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_UTILITY, 0);
        }
    }
}

#[cfg(windows)]
pub(crate) fn background_child(child: &std::process::Child) {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::Threading::{
        SetPriorityClass, SetProcessPriorityBoost, BELOW_NORMAL_PRIORITY_CLASS,
    };
    // SAFETY: Child retains ownership of its live process handle; this never
    // looks up a PID or affects the app/UI or any independently running server.
    unsafe {
        SetPriorityClass(child.as_raw_handle() as _, BELOW_NORMAL_PRIORITY_CLASS);
        SetProcessPriorityBoost(child.as_raw_handle() as _, 1);
    }
}

#[cfg(unix)]
pub(crate) fn background_command(command: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    // SAFETY: pre_exec uses only getpriority/setpriority scalar system calls.
    // After fork, this is the new private child process; on Darwin PRIO_PROCESS
    // is intentional here, unlike the per-thread QoS helper used by app workers.
    unsafe {
        command.pre_exec(|| {
            if libc::getpriority(libc::PRIO_PROCESS, 0) < 5 {
                libc::setpriority(libc::PRIO_PROCESS, 0, 5);
            }
            Ok(())
        });
    }
}

#[cfg(windows)]
fn available_memory() -> Option<u64> {
    use windows_sys::Win32::System::{
        JobObjects::{
            IsProcessInJob, JobObjectExtendedLimitInformation, QueryInformationJobObject,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_JOB_MEMORY,
            JOB_OBJECT_LIMIT_PROCESS_MEMORY,
        },
        SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX},
        Threading::GetCurrentProcess,
    };
    let mut memory: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    memory.dwLength = std::mem::size_of_val(&memory) as u32;
    // SAFETY: correctly sized writable structure.
    if unsafe { GlobalMemoryStatusEx(&mut memory) } == 0 {
        return None;
    }
    let mut available = memory.ullAvailPhys;
    let mut in_job = 0;
    // SAFETY: pseudo handle, NULL selects any current job, writable result.
    if unsafe { IsProcessInJob(GetCurrentProcess(), std::ptr::null_mut(), &mut in_job) } == 0 {
        return None;
    }
    if in_job != 0 {
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: NULL queries this process's job, correct structure and size.
        if unsafe {
            QueryInformationJobObject(
                std::ptr::null_mut(),
                JobObjectExtendedLimitInformation,
                &mut limits as *mut _ as *mut _,
                std::mem::size_of_val(&limits) as u32,
                std::ptr::null_mut(),
            )
        } == 0
        {
            return None;
        }
        // Peaks overestimate current usage: subtraction is conservative,
        // never an assertion that historical peak is current available RAM.
        // This can keep concurrency low after a high-memory operation in a
        // constrained job; it cannot inflate headroom above that job's limit.
        if limits.BasicLimitInformation.LimitFlags & JOB_OBJECT_LIMIT_JOB_MEMORY != 0 {
            available = available.min(
                limits
                    .JobMemoryLimit
                    .saturating_sub(limits.PeakJobMemoryUsed) as u64,
            );
        }
        if limits.BasicLimitInformation.LimitFlags & JOB_OBJECT_LIMIT_PROCESS_MEMORY != 0 {
            available = available.min(
                limits
                    .ProcessMemoryLimit
                    .saturating_sub(limits.PeakProcessMemoryUsed) as u64,
            );
        }
    }
    Some(available)
}

#[cfg(target_os = "linux")]
fn available_memory() -> Option<u64> {
    use std::io::Read;
    let mut text = String::new();
    std::fs::File::open("/proc/meminfo")
        .ok()?
        .take(64 * 1024)
        .read_to_string(&mut text)
        .ok()?;
    let available = mem_available(&text)?;
    let mut membership = String::new();
    std::fs::File::open("/proc/self/cgroup")
        .ok()?
        .take(64 * 1024)
        .read_to_string(&mut membership)
        .ok()?;
    let root = std::path::Path::new("/sys/fs/cgroup");
    if let Some(relative) = membership.lines().find_map(|line| line.strip_prefix("0::")) {
        if !root.is_dir() {
            return None;
        }
        return controller_headroom(root, relative, "memory.max", "memory.current", available);
    }
    let legacy = membership.lines().find_map(|line| {
        let mut fields = line.splitn(3, ':');
        fields.next()?;
        let memory = fields
            .next()?
            .split(',')
            .any(|controller| controller == "memory");
        let relative = fields.next()?;
        memory.then_some(relative)
    });
    if let Some(relative) = legacy {
        let root = root.join("memory");
        // Unknown/custom legacy mounts stay conservative instead of using host
        // RAM for a potentially small container. No mount paths are emitted.
        if !root.is_dir() {
            return None;
        }
        return controller_headroom(
            &root,
            relative,
            "memory.limit_in_bytes",
            "memory.usage_in_bytes",
            available,
        );
    }
    Some(available)
}

#[cfg(any(target_os = "linux", test))]
fn controller_headroom(
    root: &std::path::Path,
    relative: &str,
    maximum: &str,
    used: &str,
    mut available: u64,
) -> Option<u64> {
    use std::path::{Component, Path};
    let path = Path::new(relative);
    let safe = relative.starts_with('/')
        && path
            .components()
            .all(|part| matches!(part, Component::RootDir | Component::Normal(_)));
    // In a namespace the membership may name an unreachable host ancestor. A
    // finite limit at the mounted root is still a valid conservative bound.
    let mut directory = if safe {
        root.join(path.strip_prefix("/").ok()?)
    } else {
        root.to_path_buf()
    };
    let mut finite = false;
    for _ in 0..64 {
        match std::fs::read_to_string(directory.join(maximum)) {
            Ok(value) if value.trim() == "max" => {}
            Ok(value) => {
                let limit = value.trim().parse::<u64>().ok()?;
                let current = std::fs::read_to_string(directory.join(used))
                    .ok()?
                    .trim()
                    .parse::<u64>()
                    .ok()?;
                available = available.min(limit.saturating_sub(current));
                finite = true;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return None,
        }
        if directory == root {
            return (safe || finite).then_some(available);
        }
        if !directory.pop() {
            return None;
        }
    }
    None
}
#[cfg(any(target_os = "linux", test))]
fn mem_available(text: &str) -> Option<u64> {
    let mut parts = text
        .lines()
        .find_map(|line| line.strip_prefix("MemAvailable:"))?
        .split_whitespace();
    let value = parts.next()?.parse::<u64>().ok()?;
    (parts.next()? == "kB" && parts.next().is_none())
        .then(|| value.checked_mul(1024))
        .flatten()
}

#[cfg(target_os = "macos")]
fn available_memory() -> Option<u64> {
    // Declared here instead of using `libc::mach_host_self`/`mach_task_self`, which
    // the libc crate marks deprecated (it points to the mach2 crate). These are the
    // same system symbols: `mach_task_self()` in C reads the `mach_task_self_` port.
    extern "C" {
        static mach_task_self_: libc::mach_port_t;
        fn mach_host_self() -> libc::mach_port_t;
        fn mach_port_deallocate(
            task: libc::mach_port_t,
            name: libc::mach_port_t,
        ) -> libc::kern_return_t;
    }
    // SAFETY: scalar system queries and a correctly sized writable VM structure.
    unsafe {
        let page = libc::sysconf(libc::_SC_PAGESIZE);
        if page <= 0 {
            return None;
        }
        let host = mach_host_self();
        let mut stats: libc::vm_statistics64 = std::mem::zeroed();
        let mut count = libc::HOST_VM_INFO64_COUNT;
        let result = libc::host_statistics64(
            host,
            libc::HOST_VM_INFO64,
            &mut stats as *mut _ as libc::host_info64_t,
            &mut count,
        );
        mach_port_deallocate(mach_task_self_, host);
        if result != 0 || count < 4 {
            return None;
        }
        // Speculative pages are already in free_count; do not count twice.
        u64::from(stats.free_count)
            .checked_add(u64::from(stats.inactive_count))?
            .checked_mul(page as u64)
    }
}
#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn available_memory() -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn affinity_and_unknown_headroom_stay_conservative() {
        assert_eq!(cpus_allowed(64, 1), 1);
        assert_eq!(cpus_allowed(4, usize::MAX), 4);
        assert_eq!(cpus_allowed(0, 0), 1);
        assert_eq!(WorkBudget::from_available(64, None).cpu_threads(), 1);
        assert_eq!(WorkBudget::from_available(0, Some(0)).io_threads(), 1);
    }
    #[test]
    fn extra_cores_are_available_without_an_eight_worker_ceiling() {
        let low = WorkBudget::from_available(1, Some(512 * MIB));
        assert_eq!((low.cpu_threads(), low.io_threads()), (1, 1));
        let high = WorkBudget::from_available(32, Some(16 * 1024 * MIB));
        assert_eq!((high.cpu_threads(), high.io_threads()), (32, 4));
        assert_eq!(high.workers_for(512 * MIB), 8);
        let large = WorkBudget::from_available(1024, Some(64 * 1024 * MIB));
        assert_eq!(large.cpu_threads(), 1024);
        assert_eq!(large.inference_threads(), 512);
    }
    #[test]
    fn budget_is_bounded_and_monotonic_in_cpu_and_memory() {
        for cpus in [0, 1, 2, 8, 32, 128, 1024] {
            let mut previous = 1;
            for memory in [
                0,
                MIB,
                32 * MIB,
                512 * MIB,
                4 * 1024 * MIB,
                64 * 1024 * MIB,
                u64::MAX,
            ] {
                let plan = WorkBudget::from_available(cpus, Some(memory));
                assert!(plan.scratch <= MAX_SCRATCH && plan.scratch <= memory / 4);
                assert!((1..=cpus.max(1)).contains(&plan.cpu_threads()));
                assert!(plan.cpu_threads() >= previous);
                previous = plan.cpu_threads();
            }
        }
    }
    #[test]
    fn meminfo_requires_available_memory_with_explicit_units() {
        assert_eq!(
            mem_available("MemFree: 500 kB\nMemAvailable: 1234 kB\n"),
            Some(1234 * 1024)
        );
        for text in [
            "MemFree: 1 kB",
            "MemAvailable: 1 MB",
            "MemAvailable: -1 kB",
            "MemAvailable: 18446744073709551615 kB",
        ] {
            assert_eq!(mem_available(text), None);
        }
    }
    #[test]
    fn nested_controller_headroom_uses_the_tightest_ancestor_and_current_usage() {
        use std::fs;
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path();
        fs::create_dir_all(root.join("slice/worker")).unwrap();
        for (path, maximum, current) in [
            ("", "2048", "512"),
            ("slice", "1024", "128"),
            ("slice/worker", "4096", "0"),
        ] {
            fs::write(root.join(path).join("maximum"), maximum).unwrap();
            fs::write(root.join(path).join("current"), current).unwrap();
        }
        assert_eq!(
            controller_headroom(root, "/slice/worker", "maximum", "current", 5000),
            Some(896)
        );
        assert_eq!(
            controller_headroom(root, "/slice/worker", "maximum", "current", 750),
            Some(750)
        );
        fs::write(root.join("slice/current"), "2048").unwrap();
        assert_eq!(
            controller_headroom(root, "/slice/worker", "maximum", "current", 5000),
            Some(0)
        );
        fs::write(root.join("slice/current"), "not a number").unwrap();
        assert_eq!(
            controller_headroom(root, "/slice/worker", "maximum", "current", 5000),
            None
        );
    }
    #[test]
    fn unlimited_and_namespace_paths_never_escape_the_mounted_controller() {
        use std::fs;
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path();
        fs::write(root.join("maximum"), "max").unwrap();
        assert_eq!(
            controller_headroom(root, "/", "maximum", "current", 5000),
            Some(5000)
        );
        assert_eq!(
            controller_headroom(root, "/../../unreachable", "maximum", "current", 5000),
            None
        );
        fs::write(root.join("maximum"), "2048").unwrap();
        fs::write(root.join("current"), "512").unwrap();
        assert_eq!(
            controller_headroom(root, "/../../unreachable", "maximum", "current", 5000),
            Some(1536)
        );
    }
    #[cfg(windows)]
    #[test]
    #[ignore = "Owned subprocess fixture; invoked only by the child-priority test"]
    fn priority_child_fixture() {
        use std::io::Read;
        if std::env::var("PRISM_TEST_RESOURCE_PRIORITY_CHILD").as_deref() == Ok("1") {
            let mut start = [0];
            std::io::stdin().read_exact(&mut start).unwrap();
            assert_eq!(start, *b"G");
        }
    }
    #[cfg(windows)]
    #[test]
    fn background_child_changes_only_the_owned_process_and_disables_boosts() {
        use std::{
            io::Write,
            os::windows::io::AsRawHandle,
            process::{Command, Stdio},
        };
        use windows_sys::Win32::System::Threading::{
            GetCurrentProcess, GetPriorityClass, GetProcessPriorityBoost,
            BELOW_NORMAL_PRIORITY_CLASS,
        };
        struct Owned(std::process::Child);
        impl Drop for Owned {
            fn drop(&mut self) {
                if self.0.try_wait().ok().flatten().is_none() {
                    let _ = self.0.kill();
                }
                let _ = self.0.wait();
            }
        }
        // SAFETY: borrowed calling-process handle only; no mutation.
        let original = unsafe { GetPriorityClass(GetCurrentProcess()) };
        let mut child = Owned(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "resource_policy::tests::priority_child_fixture",
                    "--ignored",
                    "--test-threads=1",
                ])
                .env("PRISM_TEST_RESOURCE_PRIORITY_CHILD", "1")
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        background_child(&child.0);
        let mut disabled = 0;
        // SAFETY: the guard retains the only owned child handle and writable bool.
        unsafe {
            assert_eq!(
                GetPriorityClass(child.0.as_raw_handle() as _),
                BELOW_NORMAL_PRIORITY_CLASS
            );
            assert_ne!(
                GetProcessPriorityBoost(child.0.as_raw_handle() as _, &mut disabled),
                0
            );
            assert_eq!(GetPriorityClass(GetCurrentProcess()), original);
        }
        assert_ne!(disabled, 0);
        child.0.stdin.as_mut().unwrap().write_all(b"G").unwrap();
        assert!(child.0.wait().unwrap().success());
    }
}

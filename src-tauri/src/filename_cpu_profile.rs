//! Opt-in numeric diagnostics, never part of status/search DTOs. Windows thread
//! CPU counters exclude time blocked on locks, queues and pipe I/O. Exclusive
//! costs subtract nested scopes on the SAME thread; worker scopes have their
//! own counters. Inclusive phases and wall times must not be added together.
use serde::Serialize;
use std::{
    cell::RefCell,
    marker::PhantomData,
    rc::Rc,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex, OnceLock,
    },
    time::Instant,
};

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Phase {
    ReceiveFetch,
    ReceiveCallback,
    ReceiveQueue,
    PrepareRows,
    InsertEntries,
    Publish,
    Query,
    QueryScan,
}
const PHASES: [Phase; 8] = [
    Phase::ReceiveFetch,
    Phase::ReceiveCallback,
    Phase::ReceiveQueue,
    Phase::PrepareRows,
    Phase::InsertEntries,
    Phase::Publish,
    Phase::Query,
    Phase::QueryScan,
];

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Cost {
    wall_ns: u64,
    user_cpu_100ns: u64,
    kernel_cpu_100ns: u64,
    thread_cycles: u64,
}
impl Cost {
    fn add(&mut self, rhs: Self) {
        self.wall_ns = self.wall_ns.saturating_add(rhs.wall_ns);
        self.user_cpu_100ns = self.user_cpu_100ns.saturating_add(rhs.user_cpu_100ns);
        self.kernel_cpu_100ns = self.kernel_cpu_100ns.saturating_add(rhs.kernel_cpu_100ns);
        self.thread_cycles = self.thread_cycles.saturating_add(rhs.thread_cycles);
    }
    fn subtract(self, rhs: Self) -> Option<Self> {
        Some(Self {
            wall_ns: self.wall_ns.checked_sub(rhs.wall_ns)?,
            user_cpu_100ns: self.user_cpu_100ns.checked_sub(rhs.user_cpu_100ns)?,
            kernel_cpu_100ns: self.kernel_cpu_100ns.checked_sub(rhs.kernel_cpu_100ns)?,
            thread_cycles: self.thread_cycles.checked_sub(rhs.thread_cycles)?,
        })
    }
}
#[derive(Clone, Copy, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct Sample {
    calls: u64,
    inclusive: Cost,
    exclusive: Cost,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PhaseSample {
    phase: Phase,
    #[serde(flatten)]
    sample: Sample,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Snapshot {
    schema_version: u32,
    since_process_start: bool,
    completed_scopes_only: bool,
    lost_measurements: u64,
    phases: Vec<PhaseSample>,
}
struct Collector {
    samples: Mutex<[Sample; PHASES.len()]>,
    lost: AtomicU64,
}
impl Default for Collector {
    fn default() -> Self {
        Self {
            samples: Mutex::new([Sample::default(); PHASES.len()]),
            lost: AtomicU64::new(0),
        }
    }
}
impl Collector {
    fn record(&self, phase: Phase, inclusive: Cost, nested: Cost) {
        let Some(exclusive) = inclusive.subtract(nested) else {
            self.lost.fetch_add(1, Ordering::Relaxed);
            return;
        };
        if let Ok(mut samples) = self.samples.lock() {
            let sample = &mut samples[phase as usize];
            sample.calls = sample.calls.saturating_add(1);
            sample.inclusive.add(inclusive);
            sample.exclusive.add(exclusive);
        } else {
            self.lost.fetch_add(1, Ordering::Relaxed);
        }
    }
    fn snapshot(&self) -> Option<Snapshot> {
        let samples = self.samples.lock().ok()?;
        Some(Snapshot {
            schema_version: 1,
            since_process_start: true,
            completed_scopes_only: true,
            lost_measurements: self.lost.load(Ordering::Relaxed),
            phases: PHASES
                .into_iter()
                .zip(samples.iter().copied())
                .map(|(phase, sample)| PhaseSample { phase, sample })
                .collect(),
        })
    }
}
fn requested(value: Option<&str>) -> bool {
    value == Some("1")
}
fn collector() -> Option<&'static Collector> {
    static PROFILE: OnceLock<Option<Collector>> = OnceLock::new();
    PROFILE
        .get_or_init(|| {
            // Read once; no clocks, TLS stack, allocations or counter locks on the
            // normal disabled path after initialization. Unsupported OS stays off.
            (cfg!(windows)
                && requested(std::env::var("PRISM_PROFILE_FILENAME_CPU").ok().as_deref()))
            .then(Collector::default)
        })
        .as_ref()
}
pub(crate) fn snapshot() -> Option<Snapshot> {
    collector()?.snapshot()
}

struct Reading {
    wall: Instant,
    usage: Cost,
}
impl Reading {
    fn now() -> Option<Self> {
        Some(Self {
            usage: thread_usage()?,
            wall: Instant::now(),
        })
    }
    fn elapsed(&self) -> Option<Cost> {
        let wall = self.wall.elapsed();
        let mut cost = thread_usage()?.subtract(self.usage)?;
        cost.wall_ns = wall.as_nanos().min(u64::MAX as u128) as u64;
        Some(cost)
    }
}
#[cfg(windows)]
fn thread_usage() -> Option<Cost> {
    use windows_sys::Win32::{
        Foundation::{FILETIME, HANDLE},
        System::Threading::{GetCurrentThread, GetThreadTimes},
    };
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
    // SAFETY: current-thread pseudo handle is valid and needs no closing;
    // every output points to initialized storage owned by this call.
    if unsafe {
        GetThreadTimes(
            GetCurrentThread(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        )
    } == 0
        || unsafe { query_cycles(GetCurrentThread(), &mut cycles) } == 0
    {
        return None;
    }
    let ticks = |v: FILETIME| u64::from(v.dwHighDateTime) << 32 | u64::from(v.dwLowDateTime);
    Some(Cost {
        user_cpu_100ns: ticks(user),
        kernel_cpu_100ns: ticks(kernel),
        thread_cycles: cycles,
        ..Default::default()
    })
}
#[cfg(not(windows))]
fn thread_usage() -> Option<Cost> {
    None
}

#[derive(Default)]
struct Stack {
    next: u64,
    active: Vec<(u64, Cost)>,
}
thread_local! { static STACK: RefCell<Stack> = RefCell::new(Stack::default()); }

// Used only in synchronous scopes. !Send keeps the CPU counter and its TLS
// stack on the same thread; no guard is held across an async suspension.
pub(super) struct Scope<'a> {
    collector: &'a Collector,
    phase: Phase,
    mark: Reading,
    token: u64,
    same_thread: PhantomData<Rc<()>>,
}
pub(super) fn enter(phase: Phase) -> Option<Scope<'static>> {
    start(collector()?, phase)
}
fn start(collector: &Collector, phase: Phase) -> Option<Scope<'_>> {
    let Some(mark) = Reading::now() else {
        collector.lost.fetch_add(1, Ordering::Relaxed);
        return None;
    };
    let token = STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        stack.next = stack.next.wrapping_add(1);
        let token = stack.next;
        stack.active.push((token, Cost::default()));
        token
    });
    Some(Scope {
        collector,
        phase,
        mark,
        token,
        same_thread: PhantomData,
    })
}
impl Drop for Scope<'_> {
    fn drop(&mut self) {
        let inclusive = self.mark.elapsed();
        let nested = STACK.with(|stack| {
            let mut stack = stack.borrow_mut();
            if stack.active.last().map(|item| item.0) != Some(self.token) {
                return None;
            }
            let (_, nested) = stack.active.pop()?;
            if let (Some(parent), Some(cost)) = (stack.active.last_mut(), inclusive) {
                parent.1.add(cost);
            }
            Some(nested)
        });
        if let (Some(inclusive), Some(nested)) = (inclusive, nested) {
            self.collector.record(self.phase, inclusive, nested);
        } else {
            self.collector.lost.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_explicit_opt_in_enables_collection() {
        for value in [None, Some(""), Some("0"), Some("true"), Some("2")] {
            assert!(!requested(value));
        }
        assert!(requested(Some("1")));
    }
    #[test]
    fn nested_cpu_is_excluded_and_invalid_subtraction_is_reported() {
        let collector = Collector::default();
        let parent = Cost {
            wall_ns: 100,
            user_cpu_100ns: 50,
            kernel_cpu_100ns: 20,
            thread_cycles: 300,
        };
        let child = Cost {
            wall_ns: 40,
            user_cpu_100ns: 10,
            kernel_cpu_100ns: 5,
            thread_cycles: 100,
        };
        collector.record(Phase::ReceiveCallback, child, Cost::default());
        collector.record(Phase::ReceiveFetch, parent, child);
        let samples = collector.samples.lock().unwrap();
        let mut sum = samples[0].exclusive;
        sum.add(samples[1].exclusive);
        assert_eq!(sum, parent);
        drop(samples);
        collector.record(Phase::Query, child, parent);
        assert_eq!(collector.snapshot().unwrap().lost_measurements, 1);
    }
    #[cfg(windows)]
    #[test]
    fn actual_thread_scopes_record_nested_and_worker_work_without_paths() {
        let collector = Collector::default();
        {
            let _query = start(&collector, Phase::Query).unwrap();
            {
                let _scan = start(&collector, Phase::QueryScan).unwrap();
                for n in 0..10_000 {
                    std::hint::black_box(n * 7);
                }
            }
            std::thread::scope(|scope| {
                scope
                    .spawn(|| {
                        let _prep = start(&collector, Phase::PrepareRows).unwrap();
                        for n in 0..10_000 {
                            std::hint::black_box(n * 11);
                        }
                    })
                    .join()
                    .unwrap();
            });
        }
        let snapshot = collector.snapshot().unwrap();
        assert_eq!(snapshot.lost_measurements, 0);
        let samples = collector.samples.lock().unwrap();
        assert_eq!(samples[Phase::Query as usize].calls, 1);
        assert_eq!(samples[Phase::QueryScan as usize].calls, 1);
        assert_eq!(samples[Phase::PrepareRows as usize].calls, 1);
        assert!(samples[Phase::QueryScan as usize].inclusive.thread_cycles > 0);
        let json = serde_json::to_value(snapshot).unwrap();
        assert_eq!(json["phases"].as_array().unwrap().len(), 8);
        assert_eq!(json["completedScopesOnly"], true);
        assert!(json["phases"][0].get("path").is_none());
    }
}

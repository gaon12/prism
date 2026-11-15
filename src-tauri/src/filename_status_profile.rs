//! Developer-only wall windows. Acquisition includes scheduling; nested hold,
//! wait and total durations overlap and must not be added. No paths or queries.
use serde::Serialize;
#[cfg(any(windows, test))]
use std::sync::{MutexGuard, TryLockError, TryLockResult};
use std::{
    ops::{Deref, DerefMut},
    sync::{
        atomic::{AtomicU64, Ordering},
        LockResult, Mutex, OnceLock, PoisonError, RwLock, RwLockWriteGuard,
    },
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum WriterKind {
    Refresh,
    Metadata,
    ScanStart,
    Reconcile,
    WatchPrune,
    WalkPublish,
    ProvisionalShow,
    ProvisionalRefresh,
    ProvisionalSwap,
    ProvisionalDrop,
    InitialPublish,
    FirstBatch,
    PreparedInsert,
}
const WRITERS: [WriterKind; 13] = [
    WriterKind::Refresh,
    WriterKind::Metadata,
    WriterKind::ScanStart,
    WriterKind::Reconcile,
    WriterKind::WatchPrune,
    WriterKind::WalkPublish,
    WriterKind::ProvisionalShow,
    WriterKind::ProvisionalRefresh,
    WriterKind::ProvisionalSwap,
    WriterKind::ProvisionalDrop,
    WriterKind::InitialPublish,
    WriterKind::FirstBatch,
    WriterKind::PreparedInsert,
];
const READS: [&str; 8] = [
    "poolWait",
    "catalogWait",
    "catalogHold",
    "preparedWait",
    "preparedHold",
    "dtoBuild",
    "workerTotal",
    "commandTotal",
];

#[derive(Clone, Copy, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct Stat {
    calls: u64,
    total_ns: u64,
    max_ns: u64,
}
impl Stat {
    fn add(&mut self, value: u64) {
        self.calls = self.calls.saturating_add(1);
        self.total_ns = self.total_ns.saturating_add(value);
        self.max_ns = self.max_ns.max(value);
    }
}
#[derive(Clone, Copy, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct WriterStat {
    attempts: u64,
    acquired: u64,
    try_calls: u64,
    would_block: u64,
    poisoned: u64,
    wait: Stat,
    hold: Stat,
}
#[derive(Clone, Copy, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReadRecord {
    pool_wait_ns: Option<u64>,
    catalog_wait_ns: u64,
    catalog_hold_ns: u64,
    prepared_wait_ns: Option<u64>,
    prepared_hold_ns: Option<u64>,
    dto_build_ns: u64,
    worker_total_ns: u64,
    command_total_ns: Option<u64>,
    failed: bool,
}
#[derive(Clone, Copy, Default)]
struct State {
    read_calls: u64,
    queued_calls: u64,
    read_failures: u64,
    reads: [Stat; 8],
    writers: [WriterStat; 13],
    slowest_worker: Option<ReadRecord>,
    slowest_command: Option<ReadRecord>,
}
#[derive(Default)]
struct Collector {
    state: Mutex<State>,
    lost: AtomicU64,
}
impl Collector {
    fn read(&self, record: ReadRecord) {
        let Ok(mut state) = self.state.lock() else {
            self.lost.fetch_add(1, Ordering::Relaxed);
            return;
        };
        state.read_calls = state.read_calls.saturating_add(1);
        state.queued_calls = state
            .queued_calls
            .saturating_add(u64::from(record.pool_wait_ns.is_some()));
        state.read_failures = state.read_failures.saturating_add(u64::from(record.failed));
        for (stat, value) in state.reads.iter_mut().zip([
            record.pool_wait_ns,
            Some(record.catalog_wait_ns),
            Some(record.catalog_hold_ns),
            record.prepared_wait_ns,
            record.prepared_hold_ns,
            Some(record.dto_build_ns),
            Some(record.worker_total_ns),
            record.command_total_ns,
        ]) {
            if let Some(value) = value {
                stat.add(value);
            }
        }
        if state
            .slowest_worker
            .is_none_or(|old| record.worker_total_ns > old.worker_total_ns)
        {
            state.slowest_worker = Some(record);
        }
        if let Some(total) = record.command_total_ns {
            if state
                .slowest_command
                .is_none_or(|old| total > old.command_total_ns.unwrap_or(0))
            {
                state.slowest_command = Some(record);
            }
        }
    }
    fn writer(
        &self,
        kind: WriterKind,
        wait: u64,
        hold: Option<u64>,
        try_call: bool,
        poisoned: bool,
    ) {
        let Ok(mut state) = self.state.lock() else {
            self.lost.fetch_add(1, Ordering::Relaxed);
            return;
        };
        let stat = &mut state.writers[kind as usize];
        stat.attempts = stat.attempts.saturating_add(1);
        stat.try_calls = stat.try_calls.saturating_add(u64::from(try_call));
        stat.poisoned = stat.poisoned.saturating_add(u64::from(poisoned));
        stat.wait.add(wait);
        if let Some(hold) = hold {
            stat.acquired = stat.acquired.saturating_add(1);
            stat.hold.add(hold);
        } else {
            stat.would_block = stat.would_block.saturating_add(1);
        }
    }
    fn snapshot(&self) -> Option<Snapshot> {
        // Copy fixed-size counters, then release the recorder before allocating/serializing.
        let state = *self.state.lock().ok()?;
        Some(Snapshot {
            schema_version: 1,
            since_process_start: true,
            completed_spans_only: true,
            overlapping_durations_do_not_sum: true,
            hold_ends_before_unlock: true,
            acquisition_includes_scheduling: true,
            dto_excludes_serialization: true,
            recording_cost_excluded: true,
            lost_measurements: self.lost.load(Ordering::Relaxed),
            read_calls: state.read_calls,
            queued_calls: state.queued_calls,
            read_failures: state.read_failures,
            reads: READS
                .into_iter()
                .zip(state.reads)
                .map(|(region, stat)| ReadSummary { region, stat })
                .collect(),
            writers: WRITERS
                .into_iter()
                .zip(state.writers)
                .map(|(kind, stat)| WriterSummary { kind, stat })
                .collect(),
            slowest_worker: state.slowest_worker,
            slowest_command: state.slowest_command,
        })
    }
}
fn collector() -> Option<&'static Collector> {
    if !super::cpu_profile::enabled() {
        return None;
    }
    static STATE: OnceLock<Collector> = OnceLock::new();
    Some(STATE.get_or_init(Collector::default))
}
fn nanos(duration: Duration) -> u64 {
    duration.as_nanos().min(u64::MAX as u128) as u64
}
fn now() -> Instant {
    #[cfg(test)]
    CLOCK_READS.with(|count| count.set(count.get() + 1));
    Instant::now()
}
#[cfg(test)]
thread_local! { static CLOCK_READS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) }; }

#[derive(Serialize)]
struct ReadSummary {
    region: &'static str,
    #[serde(flatten)]
    stat: Stat,
}
#[derive(Serialize)]
struct WriterSummary {
    kind: WriterKind,
    #[serde(flatten)]
    stat: WriterStat,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Snapshot {
    schema_version: u32,
    since_process_start: bool,
    completed_spans_only: bool,
    overlapping_durations_do_not_sum: bool,
    hold_ends_before_unlock: bool,
    acquisition_includes_scheduling: bool,
    dto_excludes_serialization: bool,
    recording_cost_excluded: bool,
    lost_measurements: u64,
    read_calls: u64,
    queued_calls: u64,
    read_failures: u64,
    reads: Vec<ReadSummary>,
    writers: Vec<WriterSummary>,
    slowest_worker: Option<ReadRecord>,
    slowest_command: Option<ReadRecord>,
}
pub(crate) fn snapshot() -> Option<Snapshot> {
    collector()?.snapshot()
}

pub(crate) struct Queued {
    since: Instant,
}
pub(crate) fn queued() -> Option<Queued> {
    collector()?;
    Some(Queued { since: now() })
}
pub(crate) struct Read<'a> {
    collector: &'a Collector,
    start: Instant,
    record: ReadRecord,
    catalog_held: Option<Instant>,
    prepared_started: Option<Instant>,
    prepared_held: Option<Instant>,
    dto_started: Option<Instant>,
}
pub(crate) fn begin(queue: Option<Queued>) -> Option<Read<'static>> {
    Read::start(collector(), queue)
}
impl<'a> Read<'a> {
    fn start(collector: Option<&'a Collector>, queue: Option<Queued>) -> Option<Self> {
        let collector = collector?;
        let start = now();
        Some(Self {
            collector,
            start,
            record: ReadRecord {
                pool_wait_ns: queue
                    .map(|queue| nanos(start.saturating_duration_since(queue.since))),
                ..ReadRecord::default()
            },
            catalog_held: None,
            prepared_started: None,
            prepared_held: None,
            dto_started: None,
        })
    }
    pub(crate) fn catalog_acquired(&mut self) {
        let mark = now();
        self.record.catalog_wait_ns = nanos(mark.saturating_duration_since(self.start));
        self.catalog_held = Some(mark);
    }
    pub(crate) fn catalog_releasing(&mut self) {
        if let Some(start) = self.catalog_held.take() {
            self.record.catalog_hold_ns = nanos(now().saturating_duration_since(start));
        }
    }
    pub(crate) fn prepared_begin(&mut self) {
        self.prepared_started = Some(now());
    }
    pub(crate) fn prepared_acquired(&mut self) {
        if let Some(start) = self.prepared_started.take() {
            let mark = now();
            self.record.prepared_wait_ns = Some(nanos(mark.saturating_duration_since(start)));
            self.prepared_held = Some(mark);
        }
    }
    pub(crate) fn prepared_releasing(&mut self) {
        if let Some(start) = self.prepared_held.take() {
            self.record.prepared_hold_ns = Some(nanos(now().saturating_duration_since(start)));
        }
    }
    pub(crate) fn dto_begin(&mut self) {
        self.dto_started = Some(now());
    }
    pub(crate) fn dto_end(&mut self) {
        if let Some(start) = self.dto_started.take() {
            self.record.dto_build_ns = self
                .record
                .dto_build_ns
                .saturating_add(nanos(now().saturating_duration_since(start)));
        }
    }
    /// Caller has released both data guards. Recorder contention cannot extend them.
    pub(crate) fn finish(mut self, failed: bool) {
        self.record.worker_total_ns = nanos(now().saturating_duration_since(self.start));
        self.record.command_total_ns = self
            .record
            .pool_wait_ns
            .map(|wait| wait.saturating_add(self.record.worker_total_ns));
        self.record.failed = failed;
        self.collector.read(self.record);
    }
}

struct WriteMark<'a> {
    collector: &'a Collector,
    kind: WriterKind,
    wait: u64,
    held: Instant,
    try_call: bool,
    poisoned: bool,
}
pub(crate) struct Guard<'a, G: DerefMut> {
    inner: Option<G>,
    mark: Option<WriteMark<'a>>,
}
pub(crate) type WriteGuard<'a, T> = Guard<'a, RwLockWriteGuard<'a, T>>;
#[cfg(any(windows, test))]
pub(crate) type PreparedGuard<'a, T> = Guard<'a, MutexGuard<'a, T>>;
impl<G: DerefMut> Deref for Guard<'_, G> {
    type Target = G::Target;
    fn deref(&self) -> &Self::Target {
        self.inner.as_deref().expect("live catalog guard")
    }
}
impl<G: DerefMut> DerefMut for Guard<'_, G> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.inner.as_deref_mut().expect("live catalog guard")
    }
}
impl<G: DerefMut> Drop for Guard<'_, G> {
    fn drop(&mut self) {
        // Mark immediately BEFORE unlocking: a woken reader may preempt this
        // background thread at unlock. Recording afterward would inflate hold.
        let record = self.mark.take().map(|mark| {
            let hold = nanos(now().saturating_duration_since(mark.held));
            (mark, hold)
        });
        drop(self.inner.take());
        if let Some((mark, hold)) = record {
            mark.collector.writer(
                mark.kind,
                mark.wait,
                Some(hold),
                mark.try_call,
                mark.poisoned,
            );
        }
    }
}
fn wrapped<'a, G: DerefMut>(
    inner: G,
    collector: Option<&'a Collector>,
    started: Option<Instant>,
    kind: WriterKind,
    try_call: bool,
    poisoned: bool,
) -> Guard<'a, G> {
    let mark = collector.zip(started).map(|(collector, started)| {
        let held = now();
        WriteMark {
            collector,
            kind,
            wait: nanos(held.saturating_duration_since(started)),
            held,
            try_call,
            poisoned,
        }
    });
    Guard {
        inner: Some(inner),
        mark,
    }
}
fn write_for<'a, T>(
    lock: &'a RwLock<T>,
    collector: Option<&'a Collector>,
    kind: WriterKind,
) -> LockResult<WriteGuard<'a, T>> {
    let started = collector.map(|_| now());
    match lock.write() {
        Ok(inner) => Ok(wrapped(inner, collector, started, kind, false, false)),
        Err(error) => Err(PoisonError::new(wrapped(
            error.into_inner(),
            collector,
            started,
            kind,
            false,
            true,
        ))),
    }
}
pub(crate) fn write<T>(lock: &RwLock<T>, kind: WriterKind) -> LockResult<WriteGuard<'_, T>> {
    write_for(lock, collector(), kind)
}
#[cfg(any(windows, test))]
fn prepared_for<'a, T>(
    lock: &'a Mutex<T>,
    collector: Option<&'a Collector>,
) -> LockResult<PreparedGuard<'a, T>> {
    let started = collector.map(|_| now());
    match lock.lock() {
        Ok(inner) => Ok(wrapped(
            inner,
            collector,
            started,
            WriterKind::PreparedInsert,
            false,
            false,
        )),
        Err(error) => Err(PoisonError::new(wrapped(
            error.into_inner(),
            collector,
            started,
            WriterKind::PreparedInsert,
            false,
            true,
        ))),
    }
}
#[cfg(any(windows, test))]
pub(crate) fn prepared_write<T>(lock: &Mutex<T>) -> LockResult<PreparedGuard<'_, T>> {
    prepared_for(lock, collector())
}
#[cfg(any(windows, test))]
fn try_write_for<'a, T>(
    lock: &'a RwLock<T>,
    collector: Option<&'a Collector>,
    kind: WriterKind,
) -> TryLockResult<WriteGuard<'a, T>> {
    let started = collector.map(|_| now());
    match lock.try_write() {
        Ok(inner) => Ok(wrapped(inner, collector, started, kind, true, false)),
        Err(TryLockError::Poisoned(error)) => Err(TryLockError::Poisoned(PoisonError::new(
            wrapped(error.into_inner(), collector, started, kind, true, true),
        ))),
        Err(TryLockError::WouldBlock) => {
            if let Some((collector, start)) = collector.zip(started) {
                collector.writer(
                    kind,
                    nanos(now().saturating_duration_since(start)),
                    None,
                    true,
                    false,
                );
            }
            Err(TryLockError::WouldBlock)
        }
    }
}
#[cfg(any(windows, test))]
pub(crate) fn try_write<T>(lock: &RwLock<T>, kind: WriterKind) -> TryLockResult<WriteGuard<'_, T>> {
    try_write_for(lock, collector(), kind)
}

#[cfg(test)]
#[path = "filename_status_profile_tests.rs"]
mod tests;

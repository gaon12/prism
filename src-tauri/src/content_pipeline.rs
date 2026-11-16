//! Ordered publication with a bounded extraction window. Queued, computing,
//! completed and buffered results together never exceed the window size.
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
        Mutex,
    },
    time::Duration,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    Completed,
    Cancelled,
    Stopped,
    WorkerFailed,
}

pub(crate) struct Window<'a, T, R> {
    tasks: Option<SyncSender<(usize, T)>>,
    results: Option<Receiver<(usize, R)>>,
    work: &'a (dyn Fn(T) -> R + Sync),
    parallel: bool,
    keep_running: &'a (dyn Fn() -> bool + Sync),
    consume: &'a mut dyn FnMut(R) -> bool,
    failed: &'a AtomicBool,
    pending: BTreeMap<usize, R>,
    submitted: usize,
    consumed: usize,
    limit: usize,
    outcome: Outcome,
}
impl<T, R> Window<'_, T, R> {
    fn running(&mut self) -> bool {
        if self.failed.load(Ordering::Acquire) {
            self.outcome = Outcome::WorkerFailed;
        } else if !(self.keep_running)() {
            self.outcome = Outcome::Cancelled;
        }
        self.outcome == Outcome::Completed
    }
    pub(crate) fn push(&mut self, task: T) -> bool {
        if !self.running() {
            return false;
        }
        if !self.parallel {
            return self.apply_inline(task);
        }
        while self.submitted - self.consumed >= self.limit {
            if !self.drain_one() {
                return false;
            }
        }
        if !self.running() {
            return false;
        }
        let Some(next) = self.submitted.checked_add(1) else {
            self.outcome = Outcome::WorkerFailed;
            return false;
        };
        if self
            .tasks
            .as_ref()
            .unwrap()
            .send((self.submitted, task))
            .is_err()
        {
            self.outcome = Outcome::WorkerFailed;
            return false;
        }
        self.submitted = next;
        true
    }
    /// A seek-sensitive/unknown mounted device must not overlap any issued
    /// extraction. Publish all earlier results before reading this task inline.
    pub(crate) fn push_serial(&mut self, task: T) -> bool {
        if !self.running() {
            return false;
        }
        while self.consumed < self.submitted {
            if !self.drain_one() {
                return false;
            }
        }
        if !self.running() {
            return false;
        }
        self.apply_inline(task)
    }
    fn apply_inline(&mut self, task: T) -> bool {
        let result =
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (self.work)(task))) {
                Ok(result) => result,
                Err(_) => {
                    self.outcome = Outcome::WorkerFailed;
                    return false;
                }
            };
        if !self.running() {
            return false;
        }
        if !(self.consume)(result) {
            self.outcome = Outcome::Stopped;
            return false;
        }
        true
    }
    fn drain_one(&mut self) -> bool {
        loop {
            if !self.running() {
                return false;
            }
            if let Some(result) = self.pending.remove(&self.consumed) {
                if !(self.consume)(result) {
                    self.outcome = Outcome::Stopped;
                    return false;
                }
                self.consumed += 1;
                return true;
            }
            match self
                .results
                .as_ref()
                .unwrap()
                .recv_timeout(Duration::from_millis(25))
            {
                Ok((id, result))
                    if id >= self.consumed
                        && id < self.submitted
                        && !self.pending.contains_key(&id) =>
                {
                    self.pending.insert(id, result);
                }
                Ok(_) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.outcome = Outcome::WorkerFailed;
                    return false;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    }
    fn finish(&mut self) {
        while self.consumed < self.submitted && self.drain_one() {}
        self.running();
    }
}

fn inline<T, R>(
    keep: &(dyn Fn() -> bool + Sync),
    work: &(dyn Fn(T) -> R + Sync),
    consume: &mut dyn FnMut(R) -> bool,
    produce: impl FnOnce(&mut Window<'_, T, R>),
) -> Outcome {
    let failed = AtomicBool::new(false);
    let mut window = Window {
        tasks: None,
        results: None,
        work,
        parallel: false,
        keep_running: keep,
        consume,
        failed: &failed,
        pending: BTreeMap::new(),
        submitted: 0,
        consumed: 0,
        limit: 1,
        outcome: Outcome::Completed,
    };
    produce(&mut window);
    window.finish();
    window.outcome
}

pub(crate) fn run<T: Send, R: Send>(
    workers: usize,
    keep: impl Fn() -> bool + Sync,
    work: impl Fn(T) -> R + Sync,
    mut consume: impl FnMut(R) -> bool,
    produce: impl FnOnce(&mut Window<'_, T, R>),
) -> Outcome {
    if workers <= 1 {
        return inline(&keep, &work, &mut consume, produce);
    }
    let (tasks, receive) = mpsc::sync_channel::<(usize, T)>(workers);
    let receive = Mutex::new(receive);
    let (send, results) = mpsc::sync_channel(workers);
    let failed = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for _ in 0..workers {
            let receive = &receive;
            let send = send.clone();
            let keep = &keep;
            let work = &work;
            let failed = &failed;
            if let Ok(handle) = std::thread::Builder::new()
                .name("prism-content-extract".into())
                .spawn_scoped(scope, move || {
                    struct Failed<'a>(&'a AtomicBool);
                    impl Drop for Failed<'_> {
                        fn drop(&mut self) {
                            if std::thread::panicking() {
                                self.0.store(true, Ordering::Release);
                            }
                        }
                    }
                    let _failed = Failed(failed);
                    crate::resource_policy::background_thread();
                    loop {
                        let next = match receive.lock() {
                            Ok(receiver) => receiver.recv(),
                            Err(_) => {
                                failed.store(true, Ordering::Release);
                                break;
                            }
                        };
                        let Ok((id, task)) = next else {
                            break;
                        };
                        if !keep() {
                            break;
                        }
                        if send.send((id, work(task))).is_err() {
                            break;
                        }
                    }
                })
            {
                handles.push(handle);
            }
        }
        drop(send);
        if handles.is_empty() {
            drop(tasks);
            drop(results);
            return inline(&keep, &work, &mut consume, produce);
        }
        let mut window = Window {
            tasks: Some(tasks),
            results: Some(results),
            work: &work,
            parallel: true,
            keep_running: &keep,
            consume: &mut consume,
            failed: &failed,
            pending: BTreeMap::new(),
            submitted: 0,
            consumed: 0,
            limit: handles.len(),
            outcome: Outcome::Completed,
        };
        produce(&mut window);
        window.finish();
        let outcome = window.outcome;
        // Drop both channels before joining: send/recv waiters must unblock.
        drop(window);
        for handle in handles {
            if handle.join().is_err() {
                failed.store(true, Ordering::Release);
            }
        }
        if failed.load(Ordering::Acquire) {
            Outcome::WorkerFailed
        } else {
            outcome
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn single_worker_keeps_work_and_publication_on_the_calling_thread() {
        let caller = std::thread::current().id();
        let mut values = Vec::new();
        let outcome = run(
            1,
            || true,
            |value| {
                assert_eq!(std::thread::current().id(), caller);
                value * 2
            },
            |value| {
                values.push(value);
                true
            },
            |window| {
                for value in 0..100 {
                    assert!(window.push(value));
                }
            },
        );
        assert_eq!(outcome, Outcome::Completed);
        assert_eq!(values, (0..100).map(|v| v * 2).collect::<Vec<_>>());
    }
    #[test]
    fn a_slow_first_job_cannot_open_an_unbounded_reorder_window() {
        use std::sync::{Arc, Condvar};
        let released = Arc::new((Mutex::new(false), Condvar::new()));
        let (done, finished) = mpsc::channel();
        let allow = &released;
        let mut values = Vec::new();
        std::thread::scope(|scope| {
            scope.spawn(move || {
                for _ in 0..3 {
                    finished.recv_timeout(Duration::from_secs(3)).unwrap();
                }
                let (lock, changed) = &**allow;
                *lock.lock().unwrap() = true;
                changed.notify_all();
            });
            let result = run(
                4,
                || true,
                |id| {
                    let (lock, changed) = &**allow;
                    if id == 0 {
                        let mut ready = lock.lock().unwrap();
                        while !*ready {
                            ready = changed.wait(ready).unwrap();
                        }
                    } else if id < 4 {
                        done.send(()).unwrap();
                    } else {
                        assert!(
                            *lock.lock().unwrap(),
                            "a later job passed the full extraction window"
                        );
                    }
                    id
                },
                |id| {
                    values.push(id);
                    true
                },
                |window| {
                    for id in 0..40 {
                        assert!(window.push(id));
                    }
                },
            );
            assert_eq!(result, Outcome::Completed);
        });
        assert_eq!(values, (0..40).collect::<Vec<_>>());
    }
    #[test]
    fn serial_tasks_drain_pending_work_and_keep_global_publication_order() {
        use std::sync::{atomic::AtomicUsize, Condvar};
        let caller = std::thread::current().id();
        let active = AtomicUsize::new(0);
        let started = (Mutex::new(0usize), Condvar::new());
        let mut values = Vec::new();
        let result = run(
            2,
            || true,
            |id| {
                let current = active.fetch_add(1, Ordering::AcqRel) + 1;
                if id < 2 {
                    let (lock, changed) = &started;
                    let mut count = lock.lock().unwrap();
                    *count += 1;
                    changed.notify_all();
                    while *count < 2 {
                        let (next, wait) =
                            changed.wait_timeout(count, Duration::from_secs(5)).unwrap();
                        count = next;
                        assert!(
                            !wait.timed_out(),
                            "parallel fixture did not start both tasks"
                        );
                    }
                } else if id == 2 || id == 4 {
                    assert_eq!(std::thread::current().id(), caller);
                    assert_eq!(current, 1, "serial read overlapped another extraction");
                }
                active.fetch_sub(1, Ordering::AcqRel);
                id
            },
            |value| {
                values.push(value);
                true
            },
            |window| {
                assert!(window.push(0));
                assert!(window.push(1));
                assert!(window.push_serial(2));
                assert!(window.push(3));
                assert!(window.push_serial(4));
            },
        );
        assert_eq!(result, Outcome::Completed);
        assert_eq!(values, [0, 1, 2, 3, 4]);
        assert_eq!(active.load(Ordering::Acquire), 0);
    }
    #[test]
    fn consumer_stop_does_not_publish_later_completions() {
        let mut values = Vec::new();
        let result = run(
            4,
            || true,
            |id| id,
            |id| {
                values.push(id);
                id < 2
            },
            |window| {
                for id in 0..100 {
                    if !window.push(id) {
                        break;
                    }
                }
            },
        );
        assert_eq!(result, Outcome::Stopped);
        assert_eq!(values, [0, 1, 2]);
    }
    #[test]
    fn cancellation_discards_results_and_joins_issued_workers() {
        let cancelled = AtomicBool::new(false);
        let active = std::sync::atomic::AtomicUsize::new(0);
        let (ready, started) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let released = Mutex::new(released);
        let mut published = Vec::new();
        std::thread::scope(|scope| {
            let cancelled = &cancelled;
            scope.spawn(move || {
                started.recv_timeout(Duration::from_secs(3)).unwrap();
                cancelled.store(true, Ordering::Release);
                release.send(()).unwrap();
            });
            let result = run(
                2,
                || !cancelled.load(Ordering::Acquire),
                |id| {
                    active.fetch_add(1, Ordering::AcqRel);
                    ready.send(()).unwrap();
                    released
                        .lock()
                        .unwrap()
                        .recv_timeout(Duration::from_secs(3))
                        .unwrap();
                    active.fetch_sub(1, Ordering::AcqRel);
                    id
                },
                |id| {
                    published.push(id);
                    true
                },
                |window| {
                    assert!(window.push(0));
                    assert!(!window.push_serial(1));
                },
            );
            assert_eq!(result, Outcome::Cancelled);
            assert_eq!(active.load(Ordering::Acquire), 0);
        });
        assert!(published.is_empty());
    }
    #[test]
    fn worker_panic_is_reported_without_waiting_forever_for_its_missing_result() {
        for workers in [1, 2] {
            let result = run(
                workers,
                || true,
                |id| {
                    if id == 0 {
                        panic!("owned extraction fixture");
                    }
                    id
                },
                |_| true,
                |window| {
                    for id in 0..10 {
                        if !window.push(id) {
                            break;
                        }
                    }
                },
            );
            assert_eq!(result, Outcome::WorkerFailed);
        }
    }
}

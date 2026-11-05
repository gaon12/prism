//! A synchronous OS call may finish after its caller cancels. Own all inputs on
//! its worker and retain the admission lease until that actual call has returned.
#[cfg(target_os = "macos")]
use super::Control;
use super::{error, Code, Result};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    time::{Duration, Instant},
};

pub(super) struct Budget {
    pub cancel: Arc<AtomicBool>,
    pub deadline: Instant,
}
impl Budget {
    pub fn check(&self) -> Result<()> {
        if self.cancel.load(Ordering::Acquire) {
            Err(error(Code::Cancelled))
        } else if Instant::now() >= self.deadline {
            Err(error(Code::TimedOut))
        } else {
            Ok(())
        }
    }
}

#[cfg(target_os = "macos")]
pub(super) fn run<T: Send + 'static>(
    control: &Control<'_>,
    work: impl FnOnce(Budget) -> Result<T> + Send + 'static,
) -> Result<T> {
    control.check()?;
    owned(
        control.cancel,
        control.deadline,
        Arc::clone(&control._lease),
        work,
    )
}

fn owned<T: Send + 'static>(
    cancel: &AtomicBool,
    deadline: Instant,
    guard: impl Send + 'static,
    work: impl FnOnce(Budget) -> Result<T> + Send + 'static,
) -> Result<T> {
    let stopped = Arc::new(AtomicBool::new(false));
    let budget = Budget {
        cancel: Arc::clone(&stopped),
        deadline,
    };
    let (send, receive) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("prism-os-ocr".into())
        .spawn(move || {
            // This guard intentionally lives beyond a caller's cancellation. It
            // prevents repeated retries from creating unbounded outstanding OS work.
            let _guard = guard;
            let result = budget.check().and_then(|_| work(budget));
            let _ = send.send(result);
        })
        .map_err(|_| error(Code::Failed))?;
    loop {
        let cause = if cancel.load(Ordering::Acquire) {
            Some(Code::Cancelled)
        } else if Instant::now() >= deadline {
            Some(Code::TimedOut)
        } else {
            None
        };
        if let Some(cause) = cause {
            stopped.store(true, Ordering::Release);
            return Err(error(cause));
        }
        match receive.recv_timeout(Duration::from_millis(10)) {
            Ok(result) => {
                if cancel.load(Ordering::Acquire) {
                    return Err(error(Code::Cancelled));
                }
                if Instant::now() >= deadline {
                    return Err(error(Code::TimedOut));
                }
                return result;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err(error(Code::Failed)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    struct Guard(Arc<AtomicUsize>);
    impl Drop for Guard {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Release);
        }
    }

    #[test]
    fn cancellation_returns_without_releasing_an_unfinished_worker_guard() {
        let cancel = AtomicBool::new(false);
        let dropped = Arc::new(AtomicUsize::new(0));
        let worker_stopped = Arc::new(AtomicBool::new(false));
        let (started, wait_started) = mpsc::channel();
        let (release, wait_release) = mpsc::channel();
        std::thread::scope(|scope| {
            let cancel_ref = &cancel;
            scope.spawn(move || {
                wait_started.recv().unwrap();
                cancel_ref.store(true, Ordering::Release);
            });
            let observed = Arc::clone(&worker_stopped);
            let result: Result<()> = owned(
                &cancel,
                Instant::now() + Duration::from_secs(2),
                Guard(Arc::clone(&dropped)),
                move |budget| {
                    started.send(()).unwrap();
                    wait_release.recv().unwrap();
                    observed.store(budget.cancel.load(Ordering::Acquire), Ordering::Release);
                    Ok(())
                },
            );
            assert_eq!(result.unwrap_err().code, Code::Cancelled);
            assert_eq!(dropped.load(Ordering::Acquire), 0);
            release.send(()).unwrap();
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        while dropped.load(Ordering::Acquire) == 0 {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(worker_stopped.load(Ordering::Acquire));
        assert_eq!(dropped.load(Ordering::Acquire), 1);
    }
}

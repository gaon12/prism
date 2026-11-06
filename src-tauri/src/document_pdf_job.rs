//! One actual PDF compilation at a time. Returning early after cancellation
//! does not release admission while the worker still owns its immutable input.
use crate::document_pdf::{Code, Control, Result};
use std::{
    collections::VecDeque,
    sync::{atomic::AtomicBool, Arc, LazyLock, Mutex, Weak},
    time::{Duration, Instant},
};
struct Early {
    request: String,
    owner: String,
    at: Instant,
}
#[derive(Default)]
struct Registry {
    active: Weak<Job>,
    early: VecDeque<Early>,
}
static REGISTRY: LazyLock<Mutex<Registry>> = LazyLock::new(|| Mutex::new(Registry::default()));
fn valid(request: &str) -> bool {
    (16..=64).contains(&request.len())
        && request
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
}
#[cfg(test)]
pub(crate) fn test_guard() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Working,
    Committing,
    Saved,
    Failed,
    Cancelled(Code),
}
pub struct Job {
    request: String,
    owner: String,
    phase: Mutex<Phase>,
    pub control: Control,
}
impl Job {
    pub fn start(request: &str, owner: &str) -> Result<Arc<Self>> {
        if !valid(request) {
            return Err(Code::Input);
        }
        let mut registry = REGISTRY.lock().map_err(|_| Code::Storage)?;
        registry
            .early
            .retain(|entry| entry.at.elapsed() < Duration::from_secs(60));
        if let Some(i) = registry
            .early
            .iter()
            .position(|entry| entry.request == request && entry.owner == owner)
        {
            registry.early.remove(i);
            return Err(Code::Cancelled);
        }
        if registry.active.upgrade().is_some() {
            return Err(Code::Busy);
        }
        let job = Arc::new(Self {
            request: request.into(),
            owner: owner.into(),
            phase: Mutex::new(Phase::Working),
            control: Control {
                cancel: Arc::new(AtomicBool::new(false)),
                deadline: Instant::now() + Duration::from_secs(30),
            },
        });
        registry.active = Arc::downgrade(&job);
        Ok(job)
    }
    pub fn cancellation(&self) -> Option<Code> {
        match *self.phase.lock().unwrap_or_else(|e| e.into_inner()) {
            Phase::Cancelled(reason) => Some(reason),
            _ => None,
        }
    }
    pub fn stop(&self, reason: Code) -> bool {
        use std::sync::atomic::Ordering;
        let mut phase = self.phase.lock().unwrap_or_else(|e| e.into_inner());
        if *phase != Phase::Working {
            return false;
        }
        *phase = Phase::Cancelled(reason);
        self.control.cancel.store(true, Ordering::Release);
        true
    }
    pub fn cancel(request: &str, owner: &str) -> bool {
        if !valid(request) {
            return false;
        }
        let Ok(mut registry) = REGISTRY.lock() else {
            return false;
        };
        let job = registry.active.upgrade();
        if let Some(job) = job.filter(|job| job.request == request && job.owner == owner) {
            return job.stop(Code::Cancelled);
        }
        // A cancel IPC may overtake a not-yet-admitted export IPC. Keep only
        // bounded, short-lived request IDs, not paths or document content.
        registry.early.retain(|entry| {
            entry.at.elapsed() < Duration::from_secs(60)
                && !(entry.request == request && entry.owner == owner)
        });
        if registry.early.len() == 64 {
            registry.early.pop_front();
        }
        registry.early.push_back(Early {
            request: request.into(),
            owner: owner.into(),
            at: Instant::now(),
        });
        true
    }
    /// Cancellation and starting the atomic replace have one linearization
    /// point. Once replacement starts, report its real result rather than a
    /// misleading cancellation while the chosen file is being committed.
    pub fn commit(&self, write: impl FnOnce() -> Result<()>) -> Result<()> {
        {
            let mut phase = self.phase.lock().map_err(|_| Code::Storage)?;
            if let Phase::Cancelled(reason) = *phase {
                return Err(reason);
            }
            self.control.check()?;
            if *phase != Phase::Working {
                return Err(Code::Storage);
            }
            *phase = Phase::Committing;
        }
        let result = write();
        *self.phase.lock().unwrap_or_else(|e| e.into_inner()) = if result.is_ok() {
            Phase::Saved
        } else {
            Phase::Failed
        };
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancellation_keeps_actual_admission_and_cannot_claim_to_cancel_a_commit() {
        let _test = test_guard();
        let job = Job::start("pdf-test-request-0001", "main").unwrap();
        let worker = Arc::clone(&job);
        assert!(Job::cancel("pdf-test-request-0001", "quick"));
        assert!(Job::cancel("pdf-test-request-OTHER", "main"));
        assert_eq!(
            job.cancellation(),
            None,
            "Other tokens/windows must not cancel this worker"
        );
        assert!(Job::cancel("pdf-test-request-0001", "main"));
        drop(job);
        assert!(matches!(
            Job::start("pdf-test-request-0002", "main"),
            Err(Code::Busy)
        ));
        assert_eq!(
            worker.commit(|| panic!("Cancelled commit must not write")),
            Err(Code::Cancelled)
        );
        drop(worker);
        let job = Job::start("pdf-test-request-0002", "main").unwrap();
        job.commit(|| {
            assert!(!job.stop(Code::Cancelled));
            Ok(())
        })
        .unwrap();
        assert!(!job.stop(Code::Cancelled));
        drop(job);
        assert!(Job::cancel("pdf-test-request-EARLY", "main"));
        assert!(matches!(
            Job::start("pdf-test-request-EARLY", "main"),
            Err(Code::Cancelled)
        ));
    }
}

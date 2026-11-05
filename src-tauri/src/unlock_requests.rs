//! Password-free handshake and cancellation state for interactive PDF retries.
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
type Key = (String, String);
#[derive(Default)]
struct State {
    pending: HashMap<String, String>,
    active: HashMap<Key, Arc<AtomicBool>>,
    closed: bool,
}
#[derive(Default, Clone)]
pub(crate) struct UnlockRequests(Arc<Mutex<State>>);
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum StartError {
    Cancelled,
    Busy,
    Unavailable,
}
pub(crate) struct UnlockPermit {
    owner: UnlockRequests,
    key: Key,
    cancel: Arc<AtomicBool>,
}
impl UnlockPermit {
    pub(crate) fn cancel(&self) -> &AtomicBool {
        &self.cancel
    }
}
impl Drop for UnlockPermit {
    fn drop(&mut self) {
        self.owner
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .active
            .remove(&self.key);
    }
}
impl UnlockRequests {
    /// The caller obtains this token before sending a password. Each window
    /// retains only its latest pending token; old/replayed tokens never restart.
    pub(crate) fn prepare(&self, window: &str) -> Result<String, StartError> {
        if !matches!(window, "main" | "quick") {
            return Err(StartError::Cancelled);
        }
        let mut state = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if state.closed {
            return Err(StartError::Cancelled);
        }
        // Cancellation may still be unwinding in the dependency. Keep its slot
        // until Drop, rather than launching unlimited replacement parses.
        if state.active.len() >= 2 || state.active.keys().any(|key| key.0 == window) {
            return Err(StartError::Busy);
        }
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).map_err(|_| StartError::Unavailable)?;
        let id = format!("unlock_{:032x}", u128::from_le_bytes(bytes));
        state.pending.insert(window.to_owned(), id.clone());
        Ok(id)
    }
    pub(crate) fn start(&self, window: &str, id: &str) -> Result<UnlockPermit, StartError> {
        let key = (window.to_owned(), id.to_owned());
        let mut state = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if state.closed
            || state
                .pending
                .get(window)
                .is_none_or(|pending| pending != id)
        {
            return Err(StartError::Cancelled);
        }
        if state.active.len() >= 2 || state.active.keys().any(|key| key.0 == window) {
            return Err(StartError::Busy);
        }
        state.pending.remove(window);
        let cancel = Arc::new(AtomicBool::new(false));
        state.active.insert(key.clone(), Arc::clone(&cancel));
        Ok(UnlockPermit {
            owner: self.clone(),
            key,
            cancel,
        })
    }
    pub(crate) fn cancel(&self, window: &str, id: &str) {
        let mut state = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if state
            .pending
            .get(window)
            .is_some_and(|pending| pending == id)
        {
            state.pending.remove(window);
        }
        if let Some(cancel) = state.active.get(&(window.to_owned(), id.to_owned())) {
            cancel.store(true, Ordering::Release);
        }
    }
    pub(crate) fn cancel_all(&self, shutdown: bool) {
        let mut state = self.0.lock().unwrap_or_else(|error| error.into_inner());
        state.closed |= shutdown;
        state.pending.clear();
        for cancel in state.active.values() {
            cancel.store(true, Ordering::Release);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn early_cancel_and_replay_never_start_a_parse() {
        let requests = UnlockRequests::default();
        let early = requests.prepare("main").unwrap();
        requests.cancel("main", &early);
        assert!(matches!(
            requests.start("main", &early),
            Err(StartError::Cancelled)
        ));
        let done = requests.prepare("main").unwrap();
        drop(requests.start("main", &done).unwrap());
        assert!(matches!(
            requests.start("main", &done),
            Err(StartError::Cancelled)
        ));
        for _ in 0..1000 {
            let _ = requests.prepare("main").unwrap();
        }
        assert!(matches!(
            requests.start("main", &early),
            Err(StartError::Cancelled)
        ));
        assert_eq!(requests.0.lock().unwrap().pending.len(), 1);
    }
    #[test]
    fn cancellation_is_scoped_and_slots_survive_until_parsing_finishes() {
        let requests = UnlockRequests::default();
        let main_id = requests.prepare("main").unwrap();
        let quick_id = requests.prepare("quick").unwrap();
        let main = requests.start("main", &main_id).unwrap();
        let quick = requests.start("quick", &quick_id).unwrap();
        requests.cancel("main", &main_id);
        assert!(main.cancel().load(Ordering::Acquire));
        assert!(!quick.cancel().load(Ordering::Acquire));
        assert_eq!(requests.prepare("main"), Err(StartError::Busy));
        drop(main);
        let next = requests.prepare("main").unwrap();
        let replacement = requests.start("main", &next).unwrap();
        requests.cancel_all(false);
        assert!(replacement.cancel().load(Ordering::Acquire));
        assert!(quick.cancel().load(Ordering::Acquire));
        drop((replacement, quick));
        assert!(requests.prepare("main").is_ok());
    }
    #[test]
    fn new_handshakes_and_shutdown_invalidate_old_commands() {
        let requests = UnlockRequests::default();
        let old = requests.prepare("main").unwrap();
        let current = requests.prepare("main").unwrap();
        requests.cancel("main", &old);
        assert!(matches!(
            requests.start("main", &old),
            Err(StartError::Cancelled)
        ));
        let permit = requests.start("main", &current).unwrap();
        requests.cancel_all(true);
        assert!(permit.cancel().load(Ordering::Acquire));
        drop(permit);
        assert_eq!(requests.prepare("main"), Err(StartError::Cancelled));
    }
}

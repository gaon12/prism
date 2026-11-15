//! Last committed progress only. Search/RAG continue to use canonical storage.
use super::{status_profile, Catalog, FileIndex, IndexStatus};
use std::{
    ops::{Deref, DerefMut},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

/// Separate from the row arena: retaining a progress token never retains rows.
#[derive(Default)]
pub(super) struct Progress {
    count: AtomicUsize,
}
impl Progress {
    #[cfg(any(windows, test))]
    pub(super) fn inserted(&self, count: usize) {
        // Workers release the prepared mutex before publishing. A later batch
        // may publish first; counts only increase within this exact attempt.
        self.count.fetch_max(count, Ordering::Release);
    }
}

#[derive(Default)]
struct Published {
    status: Arc<IndexStatus>,
    provisional: Option<(u64, Arc<Progress>)>,
}
#[derive(Default)]
pub(super) struct Store {
    state: Mutex<Published>,
}
impl Store {
    fn publish(&self, index: &FileIndex, catalog: &Catalog) {
        let generation = catalog.generation;
        if index.generation.load(Ordering::SeqCst) != generation {
            return;
        }
        let provisional = catalog
            .provisional
            .as_ref()
            .filter(|source| catalog.entries.is_empty() && source.generation == generation);
        let mut status = catalog.status.clone();
        // This is the fallback after revocation; an active token overlays its
        // latest completed count in the same short snapshot-read critical section.
        if provisional.is_some() {
            status.count = catalog.entries.len();
        }
        let next = Published {
            status: Arc::new(status),
            provisional: provisional.map(|source| (generation, source.progress.clone())),
        };
        let old = {
            let Ok(mut state) = self.state.lock() else {
                return;
            };
            // Refresh can revoke a writer while it is building the Arc above.
            if index.generation.load(Ordering::SeqCst) != generation {
                return;
            }
            std::mem::replace(&mut *state, next)
        };
        // Free the preceding DTO/token outside the status mutex.
        drop(old);
    }

    pub(super) fn read(
        &self,
        index: &FileIndex,
        timing: &mut Option<status_profile::Read<'_>>,
    ) -> Result<IndexStatus, String> {
        // RwLock's poison flag is atomic; this does not acquire its data lock.
        if index.catalog.is_poisoned() {
            return Err("File index unavailable.".into());
        }
        if let Some(timing) = timing.as_mut() {
            timing.snapshot_begin();
        }
        let state = self.state.lock().map_err(|_| "File index unavailable.")?;
        if let Some(timing) = timing.as_mut() {
            timing.snapshot_acquired();
        }
        let snapshot = state.status.clone();
        let count = state.provisional.as_ref().map(|(generation, progress)| {
            if index.current(*generation) {
                progress.count.load(Ordering::Acquire)
            } else {
                // Revocation must hide unfinished rows even before its writer
                // can detach the prepared catalog behind a running search.
                snapshot.count
            }
        });
        if let Some(timing) = timing.as_mut() {
            timing.snapshot_releasing();
        }
        drop(state);
        if let Some(timing) = timing.as_mut() {
            timing.dto_begin();
        }
        let mut result = (*snapshot).clone();
        if let Some(count) = count {
            result.count = count;
        }
        if let Some(timing) = timing.as_mut() {
            timing.dto_end();
        }
        Ok(result)
    }
}

/// Every canonical writer commits progress, including early returns. The
/// status mutex is never held while acquiring a data lock or running I/O.
pub(super) struct CatalogGuard<'a> {
    index: &'a FileIndex,
    inner: status_profile::WriteGuard<'a, Catalog>,
}
impl<'a> CatalogGuard<'a> {
    pub(super) fn new(
        index: &'a FileIndex,
        inner: status_profile::WriteGuard<'a, Catalog>,
    ) -> Self {
        Self { index, inner }
    }
}
impl Deref for CatalogGuard<'_> {
    type Target = Catalog;
    fn deref(&self) -> &Catalog {
        &self.inner
    }
}
impl DerefMut for CatalogGuard<'_> {
    fn deref_mut(&mut self) -> &mut Catalog {
        &mut self.inner
    }
}
impl Drop for CatalogGuard<'_> {
    fn drop(&mut self) {
        // Never publish a partially modified state during panic unwinding.
        if !std::thread::panicking() && !self.index.catalog.is_poisoned() {
            self.index.published.publish(self.index, &self.inner);
        }
    }
}

#[cfg(test)]
#[path = "filename_status_tests.rs"]
mod tests;

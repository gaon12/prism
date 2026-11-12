//! Bounded, private preparation while the authenticated client receives frames.
//! Only the caller may publish, after the complete stream and root validate.
use super::{catalog::*, *};
use prism_ntfs_protocol::{FileRecord, MAX_BATCH_RECORDS};
use std::thread::{Scope, ScopedJoinHandle};

pub(super) struct Preparation<'scope> {
    index: &'scope FileIndex,
    generation: u64,
    sender: Option<mpsc::SyncSender<Vec<FileRecord>>>,
    workers: Vec<ScopedJoinHandle<'scope, io::Result<Prepared>>>,
    /// The one private catalog all workers add their prepared batches to.
    /// Unicode work runs in parallel outside the lock; only the cheap compact
    /// insertion is serialized, and no whole-catalog merge remains at the end.
    prepared: Arc<Mutex<Prepared>>,
    stop: Arc<AtomicBool>,
    #[cfg(test)]
    processed: Arc<std::sync::atomic::AtomicUsize>,
}

impl<'scope> Preparation<'scope> {
    pub(super) fn start<'env>(
        scope: &'scope Scope<'scope, 'env>,
        index: &'env FileIndex,
        generation: u64,
        sweep: u64,
        workers: usize,
    ) -> io::Result<Self> {
        if !(1..=8).contains(&workers) {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        if !index.current(generation) {
            return Err(cancelled());
        }
        // At most one incoming frame per queued slot. Each worker owns at most
        // one further batch; no second whole-snapshot raw-row buffer is kept.
        let (sender, receiver) = mpsc::sync_channel::<Vec<FileRecord>>(workers);
        let receiver = Arc::new(Mutex::new(receiver));
        let mut preparation = Self {
            index,
            generation,
            sender: Some(sender),
            workers: Vec::new(),
            prepared: Arc::new(Mutex::new(Prepared::new())),
            stop: Arc::new(AtomicBool::new(false)),
            #[cfg(test)]
            processed: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        };
        for _ in 0..workers {
            let receiver = receiver.clone();
            let stop = preparation.stop.clone();
            let shared = preparation.prepared.clone();
            #[cfg(test)]
            let processed = preparation.processed.clone();
            let worker = std::thread::Builder::new()
                .name("prism-file-stream".into())
                .spawn_scoped(scope, move || {
                    let mut ready = Vec::new();
                    loop {
                        if !active(index, generation, &stop) {
                            return Err(cancelled());
                        }
                        let batch = receiver
                            .lock()
                            .unwrap_or_else(|error| error.into_inner())
                            .recv();
                        let Ok(batch) = batch else { break };
                        #[cfg(test)]
                        let count = batch.len();
                        for (position, record) in batch.into_iter().enumerate() {
                            if position % 512 == 0 && !active(index, generation, &stop) {
                                return Err(cancelled());
                            }
                            ready.push(Entry::new(raw(record), sweep));
                        }
                        let mut map = shared.lock().unwrap_or_else(|error| error.into_inner());
                        for entry in ready.drain(..) {
                            insert(&mut map, entry);
                        }
                        #[cfg(test)]
                        processed.fetch_add(count, Ordering::Release);
                    }
                    // Everything went to the shared catalog.
                    Ok(Prepared::new())
                })?;
            preparation.workers.push(worker);
        }
        Ok(preparation)
    }

    pub(super) fn send(&self, rows: Vec<FileRecord>) -> io::Result<()> {
        if rows.len() <= MAX_BATCH_RECORDS {
            return self.send_batch(rows);
        }
        // Production frames already have this bound. Also keep injected test
        // batches bounded without cloning their owned paths or changing order.
        let mut rows = rows.into_iter();
        loop {
            let batch: Vec<_> = rows.by_ref().take(MAX_BATCH_RECORDS).collect();
            if batch.is_empty() {
                return Ok(());
            }
            self.send_batch(batch)?;
        }
    }

    fn send_batch(&self, mut batch: Vec<FileRecord>) -> io::Result<()> {
        loop {
            if !active(self.index, self.generation, &self.stop) {
                return Err(cancelled());
            }
            match self
                .sender
                .as_ref()
                .ok_or_else(worker_error)?
                .try_send(batch)
            {
                Ok(()) => return Ok(()),
                Err(mpsc::TrySendError::Full(value)) => {
                    batch = value;
                    // Backpressure must still notice a scope/owner change;
                    // a blocking send would hide it behind a busy worker.
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(mpsc::TrySendError::Disconnected(_)) => return Err(worker_error()),
            }
        }
    }

    pub(super) fn finish(mut self) -> io::Result<Prepared> {
        self.sender.take();
        join_all(std::mem::take(&mut self.workers), &self.stop)?;
        // All workers are joined: nobody else holds the shared catalog lock.
        let prepared = std::mem::take(
            &mut *self
                .prepared
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
        );
        if !active(self.index, self.generation, &self.stop) {
            return Err(cancelled());
        }
        Ok(prepared)
    }
}

impl Drop for Preparation<'_> {
    fn drop(&mut self) {
        // Fetch, terminal validation or a producer callback may fail or panic.
        // Close the queue, stop and join the actual workers on every path. No
        // extra FileIndex lifetime owners keep a revoked caller falsely alive.
        self.stop.store(true, Ordering::Release);
        self.sender.take();
        let _ = join_all(std::mem::take(&mut self.workers), &self.stop);
    }
}

#[cfg(test)]
#[path = "filename_pipeline_tests.rs"]
mod tests;

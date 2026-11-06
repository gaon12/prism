//! Prepare a first validated filename snapshot in private trees. Workers own
//! their maps, so projections and tree insertion do not contend on the live
//! catalog. Publication still belongs to the current generation and owner.
use super::*;
use prism_ntfs_protocol::FileRecord;

pub(super) type Prepared = BTreeMap<Arc<str>, Entry>;

pub(super) fn raw(record: FileRecord) -> Raw {
    Raw {
        key: path_key_text(&record.path),
        has_name: record
            .path
            .rsplit(['\\', '/'])
            .next()
            .is_some_and(|name| !name.is_empty()),
        path: record.path,
        is_dir: record.is_dir,
        size: record.size,
        modified_ms: record.modified_ms,
    }
}
pub(super) fn cancelled() -> io::Error {
    io::Error::new(
        io::ErrorKind::ConnectionAborted,
        "Filename preparation cancelled.",
    )
}
pub(super) fn worker_error() -> io::Error {
    io::Error::other("Filename preparation worker failed.")
}
pub(super) fn active(index: &FileIndex, generation: u64, stop: &AtomicBool) -> bool {
    !stop.load(Ordering::Acquire) && index.current(generation)
}
pub(super) fn join_all(
    handles: Vec<std::thread::ScopedJoinHandle<'_, io::Result<Prepared>>>,
    stop: &AtomicBool,
) -> io::Result<Vec<Prepared>> {
    let mut maps = Vec::new();
    let mut failure = None;
    for handle in handles {
        match handle
            .join()
            .map_err(|_| worker_error())
            .and_then(|value| value)
        {
            Ok(map) => maps.push(map),
            Err(error) => {
                stop.store(true, Ordering::Release);
                if failure.is_none() {
                    failure = Some(error);
                }
            }
        }
    }
    // Even an early error retains all real joins. Other successful maps stay
    // private and are dropped rather than being published as partial results.
    if let Some(error) = failure {
        Err(error)
    } else {
        Ok(maps)
    }
}
pub(super) fn insert(map: &mut Prepared, mut entry: Entry) {
    use std::collections::btree_map::Entry as Slot;
    match map.entry(entry.key.clone()) {
        Slot::Vacant(slot) => {
            slot.insert(entry);
        }
        Slot::Occupied(mut slot) => {
            entry.key = slot.key().clone();
            slot.insert(entry);
        }
    }
}
fn merge(mut left: Prepared, mut right: Prepared) -> Prepared {
    let before = left.len() + right.len();
    left.append(&mut right);
    if left.len() < before {
        // Duplicate identities are unusual in a real MFT stream. Preserve the
        // shared key allocation even if append chooses a different equal key.
        for (key, entry) in &mut left {
            if !Arc::ptr_eq(key, &entry.key) {
                entry.key = key.clone();
            }
        }
    }
    left
}

impl FileIndex {
    #[cfg(test)]
    pub(super) fn prepare_initial_catalog(
        &self,
        generation: u64,
        rows: Vec<Vec<FileRecord>>,
        sweep: u64,
        workers: usize,
    ) -> io::Result<Prepared> {
        if !(1..=8).contains(&workers) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid filename worker count.",
            ));
        }
        if !self.current(generation) {
            return Err(cancelled());
        }
        let stop = AtomicBool::new(false);
        let jobs = Mutex::new(rows.into_iter().flatten());
        let maps = std::thread::scope(|scope| {
            let mut handles = Vec::new();
            let mut spawn_error = None;
            for _ in 0..workers {
                let jobs = &jobs;
                let stop = &stop;
                let handle = std::thread::Builder::new()
                    .name("prism-file-prepare".into())
                    .spawn_scoped(scope, move || {
                        let mut map = Prepared::new();
                        loop {
                            if !active(self, generation, stop) {
                                return Err(cancelled());
                            }
                            let batch: Vec<_> = jobs
                                .lock()
                                .unwrap_or_else(|error| error.into_inner())
                                .by_ref()
                                .take(512)
                                .collect();
                            if batch.is_empty() {
                                break;
                            }
                            for record in batch {
                                insert(&mut map, Entry::new(raw(record), sweep));
                            }
                        }
                        Ok(map)
                    });
                match handle {
                    Ok(handle) => handles.push(handle),
                    Err(error) => {
                        stop.store(true, Ordering::Release);
                        spawn_error = Some(error);
                        break;
                    }
                }
            }
            let result = join_all(handles, &stop);
            if let Some(error) = spawn_error {
                Err(error)
            } else {
                result
            }
        })?;
        self.merge_initial_maps(generation, maps, &stop)
    }

    pub(super) fn merge_initial_maps(
        &self,
        generation: u64,
        mut maps: Vec<Prepared>,
        stop: &AtomicBool,
    ) -> io::Result<Prepared> {
        while maps.len() > 1 {
            if !active(self, generation, stop) {
                return Err(cancelled());
            }
            maps = std::thread::scope(|scope| {
                let mut remaining = maps.into_iter();
                let mut handles = Vec::new();
                let mut carry = None;
                let mut spawn_error = None;
                while let Some(left) = remaining.next() {
                    let Some(right) = remaining.next() else {
                        carry = Some(left);
                        break;
                    };
                    let handle = std::thread::Builder::new()
                        .name("prism-file-merge".into())
                        .spawn_scoped(scope, move || {
                            if !active(self, generation, stop) {
                                return Err(cancelled());
                            }
                            let map = merge(left, right);
                            // A standard-tree merge cannot be interrupted in
                            // its interior. Keep its real work private and
                            // joined, then recheck before any publication.
                            if !active(self, generation, stop) {
                                return Err(cancelled());
                            }
                            Ok(map)
                        });
                    match handle {
                        Ok(handle) => handles.push(handle),
                        Err(error) => {
                            stop.store(true, Ordering::Release);
                            spawn_error = Some(error);
                            break;
                        }
                    }
                }
                let result = join_all(handles, stop);
                if let Some(error) = spawn_error {
                    return Err(error);
                }
                let mut maps = result?;
                if let Some(carry) = carry {
                    maps.push(carry);
                }
                Ok(maps)
            })?;
        }
        if !active(self, generation, stop) {
            return Err(cancelled());
        }
        Ok(maps.pop().unwrap_or_default())
    }

    pub(super) fn publish_initial_catalog(
        &self,
        generation: u64,
        prepared: Prepared,
    ) -> io::Result<()> {
        let mut catalog = self.catalog.write().map_err(|_| worker_error())?;
        if !self.current(generation) {
            return Err(cancelled());
        }
        if catalog.entries.is_empty() {
            if !prepared.is_empty() {
                catalog.entries = prepared;
                catalog.status.revision += 1;
            }
            catalog.status.count = catalog.entries.len();
            catalog.status.scanning = true;
            return Ok(());
        }
        // Another valid writer may have populated the catalog after our empty
        // check. Retain those entries and unchanged identity allocations with
        // the existing bounded publication path, checking each batch again.
        drop(catalog);
        let mut entries = prepared.into_values();
        loop {
            let batch: Vec<_> = entries.by_ref().take(512).collect();
            if batch.is_empty() {
                break;
            }
            self.publish_first_batch(generation, batch)?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "filename_catalog_tests.rs"]
mod tests;

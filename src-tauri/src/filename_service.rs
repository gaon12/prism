//! Optional snapshot source for complete local drives. A reply stays private
//! until the client has validated its terminal frame and open root identity.
//! Ordinary folder walks remain available on every OS and on service failure.
use super::*;
#[cfg(any(windows, test))]
use prism_ntfs_protocol::stream::Event;
#[cfg(any(windows, test))]
use std::io;

impl FileIndex {
    #[cfg(not(windows))]
    pub(super) fn try_service_scan(&self, _: u64, _: &Path, _: u64) -> bool {
        false
    }

    #[cfg(windows)]
    pub(super) fn try_service_scan(&self, generation: u64, root: &Path, sweep: u64) -> bool {
        use prism_ntfs_windows::{client, scm};
        if !self.current(generation) {
            return true;
        }
        // The probe is read-only. Stopped/missing services never cause an
        // implicit start, privilege change or an elevation prompt.
        if client::root_identity(root).is_err() || !matches!(scm::state(), Ok(scm::State::Running))
        {
            return false;
        }
        match self.collect_service_snapshot(generation, root, sweep, |cancel, receive| {
            client::snapshot(root, cancel, receive)
        }) {
            Ok(()) => true,
            Err(_) if !self.current(generation) => true,
            Err(_) => {
                // Do not display service internals or paths from rejected
                // responses. A normal walk starts immediately for this root.
                if let Ok(mut catalog) = self.catalog.write() {
                    if self.current(generation) && catalog.status.warnings.len() < 5 {
                        catalog
                            .status
                            .warnings
                            .push("NTFS service unavailable; using folder collection.".into());
                    }
                }
                false
            }
        }
    }

    #[cfg(any(windows, test))]
    fn collect_service_snapshot(
        &self,
        generation: u64,
        root: &Path,
        sweep: u64,
        fetch: impl FnOnce(Arc<AtomicBool>, &mut dyn FnMut(Event) -> io::Result<()>) -> io::Result<u64>,
    ) -> io::Result<()> {
        let root_key = path_key(root);
        let cancel = Arc::new(AtomicBool::new(false));
        let mut rows = Vec::new();
        let result = std::thread::scope(|scope| {
            let (stop, stopped) = mpsc::channel::<()>();
            let cancel = &cancel;
            scope.spawn(move || {
                while matches!(
                    stopped.recv_timeout(Duration::from_millis(25)),
                    Err(mpsc::RecvTimeoutError::Timeout)
                ) {
                    if !self.current(generation) {
                        cancel.store(true, Ordering::Release);
                        break;
                    }
                }
            });
            let result = fetch(cancel.clone(), &mut |event| {
                if !self.current(generation) {
                    return Err(io::Error::new(
                        io::ErrorKind::ConnectionAborted,
                        "Collection cancelled.",
                    ));
                }
                match event {
                    Event::Records(records) => {
                        for record in records {
                            let path = PathBuf::from(record.path);
                            let key = path_key(&path);
                            if !under(&key, &root_key) {
                                return Err(io::Error::new(
                                    io::ErrorKind::InvalidData,
                                    "Out-of-scope filename response.",
                                ));
                            }
                            rows.push(Raw {
                                key,
                                name: path
                                    .file_name()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                                    .into_owned(),
                                path,
                                is_dir: record.is_dir,
                                size: record.size,
                                modified_ms: record.modified_ms,
                            });
                        }
                    }
                    Event::Progress { .. } => {}
                    Event::Finished { .. } | Event::Removed(_) => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "Unexpected snapshot event.",
                        ));
                    }
                }
                Ok(())
            });
            drop(stop);
            result
        });
        if result? == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid snapshot revision.",
            ));
        }
        if !self.current(generation) {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "Collection cancelled.",
            ));
        }
        // Derive Unicode projections and publish using the same bounded batches
        // as the walker. A truncated, rejected or cancelled stream inserts none.
        let rows = Mutex::new(rows.into_iter());
        let threads = std::thread::available_parallelism()
            .map_or(2, usize::from)
            .clamp(1, 8);
        std::thread::scope(|scope| {
            for _ in 0..threads {
                let rows = &rows;
                scope.spawn(move || loop {
                    if !self.current(generation) {
                        break;
                    }
                    let mut batch: Vec<_> = rows
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .by_ref()
                        .take(512)
                        .collect();
                    if batch.is_empty() {
                        break;
                    }
                    self.publish(generation, sweep, &mut batch, 0, false);
                });
            }
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prism_ntfs_protocol::FileRecord;

    // Unit-only input. No pipe impersonation, volume read or running service;
    // native authentication is covered in the independent indexer workspace.
    fn isolated() -> (FileIndex, FileIndex) {
        let (tx, _) = mpsc::channel();
        let index = FileIndex {
            catalog: Arc::new(RwLock::new(Catalog::default())),
            generation: Arc::new(AtomicU64::new(1)),
            tx,
            lifetime: Arc::new(()),
        };
        (index.clone(), index)
    }
    fn record(root: &Path, name: &str) -> FileRecord {
        FileRecord {
            path: root.join(name).to_string_lossy().into_owned(),
            is_dir: false,
            size: 42,
            modified_ms: Some(17),
        }
    }

    #[test]
    fn complete_snapshots_publish_metadata_and_unchanged_sweeps_keep_revision() {
        let (index, _owner) = isolated();
        let dir = tempfile::tempdir().unwrap();
        let row = record(dir.path(), "한글.md");
        for sweep in [1, 2] {
            index
                .collect_service_snapshot(1, dir.path(), sweep, |_, receive| {
                    receive(Event::Records(vec![row.clone()]))?;
                    if sweep == 1 {
                        assert_eq!(
                            index.status().unwrap().count,
                            0,
                            "private before validation"
                        );
                    }
                    Ok(1)
                })
                .unwrap();
        }
        let catalog = index.catalog.read().unwrap();
        assert_eq!(catalog.status.revision, 1);
        assert_eq!(catalog.entries.len(), 1);
        let entry = catalog.entries.values().next().unwrap();
        assert_eq!(entry.name(), "한글.md");
        assert_eq!(entry.size, 42);
        assert_eq!(entry.modified_ms, Some(17));
        assert_eq!(entry.seen, 2);
    }

    #[test]
    fn partial_or_out_of_scope_streams_insert_nothing() {
        let (index, _owner) = isolated();
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        index
            .collect_service_snapshot(1, dir.path(), 1, |_, receive| {
                receive(Event::Records(vec![record(dir.path(), "existing.md")]))?;
                Ok(1)
            })
            .unwrap();
        for fail_after_rows in [true, false] {
            let result = index.collect_service_snapshot(1, dir.path(), 1, |_, receive| {
                receive(Event::Records(vec![record(dir.path(), "private.md")]))?;
                if fail_after_rows {
                    return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "partial"));
                }
                receive(Event::Records(vec![record(outside.path(), "outside.md")]))?;
                Ok(1)
            });
            assert!(result.is_err());
            assert_eq!(index.status().unwrap().count, 1);
            assert_eq!(index.status().unwrap().revision, 1);
            assert_eq!(
                index
                    .catalog
                    .read()
                    .unwrap()
                    .entries
                    .values()
                    .next()
                    .unwrap()
                    .name(),
                "existing.md"
            );
        }
    }

    #[test]
    fn revocation_cancels_an_idle_fetch_without_publishing_pending_rows() {
        let (index, _owner) = isolated();
        let dir = tempfile::tempdir().unwrap();
        let result = index.collect_service_snapshot(1, dir.path(), 1, |cancel, receive| {
            receive(Event::Records(vec![record(dir.path(), "pending.md")]))?;
            index.generation.fetch_add(1, Ordering::SeqCst);
            let start = std::time::Instant::now();
            while !cancel.load(Ordering::Acquire) && start.elapsed() < Duration::from_secs(2) {
                std::thread::sleep(Duration::from_millis(5));
            }
            assert!(
                cancel.load(Ordering::Acquire),
                "idle native read must be cancelled"
            );
            Ok(1)
        });
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::ConnectionAborted);
        assert_eq!(index.status().unwrap().count, 0);
    }

    #[test]
    fn parallel_publication_keeps_all_rows_under_the_validated_root() {
        let (index, _owner) = isolated();
        let dir = tempfile::tempdir().unwrap();
        index
            .collect_service_snapshot(1, dir.path(), 9, |_, receive| {
                for batch in 0..8 {
                    receive(Event::Records(
                        (0..1024)
                            .map(|n| record(dir.path(), &format!("문서-{batch}-{n}.md")))
                            .collect(),
                    ))?;
                }
                Ok(1)
            })
            .unwrap();
        let catalog = index.catalog.read().unwrap();
        assert_eq!(catalog.entries.len(), 8192);
        assert_eq!(catalog.status.count, 8192);
        assert!(catalog.entries.values().all(|entry| entry.seen == 9));
    }
}

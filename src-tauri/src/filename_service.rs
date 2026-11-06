//! Optional snapshot source for complete local drives. A reply stays private
//! until the client has validated its terminal frame and open root identity.
//! Ordinary folder walks remain available on every OS and on service failure.
use super::*;
#[cfg(any(windows, test))]
use prism_ntfs_protocol::stream::Event;
#[cfg(any(windows, test))]
use std::io;
#[cfg(any(windows, test))]
#[path = "filename_catalog.rs"]
mod catalog;

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
        let drive = (root_key.len() == 2 && root_key.as_bytes()[1] == b':')
            .then(|| prism_ntfs_protocol::DriveLetter::new(root_key.as_bytes()[0]).ok())
            .flatten();
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
                        for record in &records {
                            // Whole-drive validation needs no allocated path
                            // key. Keep decoded batches private and defer path
                            // conversion/normalization to publication workers.
                            let inside = drive.map_or_else(
                                || under(&path_key(Path::new(&record.path)), &root_key),
                                |drive| drive.contains(&record.path),
                            );
                            if !inside {
                                return Err(io::Error::new(
                                    io::ErrorKind::InvalidData,
                                    "Out-of-scope filename response.",
                                ));
                            }
                        }
                        rows.push(records);
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
        // Prepare Unicode projections only after terminal/root validation. A
        // first large catalog is built privately; existing entries keep their
        // bounded reuse path. Rejected or cancelled streams insert nothing.
        let initial = self
            .catalog
            .read()
            .is_ok_and(|catalog| catalog.entries.is_empty());
        let threads = std::thread::available_parallelism()
            .map_or(2, usize::from)
            .clamp(1, 8);
        if initial && threads > 1 && rows.iter().map(Vec::len).sum::<usize>() >= 4096 {
            let prepared = self.prepare_initial_catalog(generation, rows, sweep, threads)?;
            return self.publish_initial_catalog(generation, prepared);
        }
        self.publish_service_rows(generation, rows, sweep, initial, threads)
    }

    #[cfg(any(windows, test))]
    fn publish_service_rows(
        &self,
        generation: u64,
        rows: Vec<Vec<prism_ntfs_protocol::FileRecord>>,
        sweep: u64,
        initial: bool,
        threads: usize,
    ) -> io::Result<()> {
        let rows = Mutex::new(rows.into_iter().flatten());
        std::thread::scope(|scope| {
            let mut workers = Vec::new();
            for _ in 0..threads {
                let rows = &rows;
                workers.push(scope.spawn(move || -> io::Result<()> {
                    loop {
                        if !self.current(generation) {
                            break;
                        }
                        let records: Vec<_> = rows
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .by_ref()
                            .take(512)
                            .collect();
                        if records.is_empty() {
                            break;
                        }
                        let mut batch: Vec<_> = records.into_iter().map(catalog::raw).collect();
                        if initial {
                            let entries = batch
                                .into_iter()
                                .map(|raw| Entry::new(raw, sweep))
                                .collect();
                            self.publish_first_batch(generation, entries)?;
                        } else {
                            self.publish(generation, sweep, &mut batch, 0, false);
                        }
                    }
                    Ok(())
                }));
            }
            workers
                .into_iter()
                .try_for_each(|worker| worker.join().expect("Filename preparation worker"))
        })
    }

    #[cfg(any(windows, test))]
    fn publish_first_batch(&self, generation: u64, entries: Vec<Entry>) -> io::Result<()> {
        use std::collections::btree_map::Entry as Slot;
        let mut catalog = self
            .catalog
            .write()
            .map_err(|_| io::Error::other("Filename catalog unavailable."))?;
        if !self.current(generation) {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "Collection cancelled.",
            ));
        }
        // The first scan has no prior metadata/projections to reuse. Prepare
        // its bounded batch outside the lock, then use one tree lookup per row
        // instead of a read lookup followed by a second insertion lookup.
        let mut changed = false;
        for entry in entries {
            match catalog.entries.entry(entry.key.clone()) {
                Slot::Vacant(slot) => {
                    slot.insert(entry);
                    changed = true;
                }
                Slot::Occupied(mut slot) => {
                    let previous = slot.get_mut();
                    if previous.path == entry.path
                        && previous.is_dir == entry.is_dir
                        && previous.size == entry.size
                        && previous.modified_ms == entry.modified_ms
                    {
                        previous.seen = entry.seen;
                    } else {
                        slot.insert(entry);
                        changed = true;
                    }
                }
            }
        }
        catalog.status.count = catalog.entries.len();
        catalog.status.scanning = true;
        if changed {
            catalog.status.revision += 1;
        }
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
    fn whole_drive_fast_validation_rejects_escapes_before_any_parallel_publication() {
        let (index, _owner) = isolated();
        for invalid in [
            r"D:\outside.md",
            r"C:\..\escape.md",
            r"C:\file:stream",
            r"C:/wrong-separator.md",
        ] {
            let result = index.collect_service_snapshot(1, Path::new(r"C:\"), 1, |_, receive| {
                receive(Event::Records(vec![FileRecord {
                    path: r"C:\valid.md".into(),
                    is_dir: false,
                    size: 0,
                    modified_ms: None,
                }]))?;
                receive(Event::Records(vec![FileRecord {
                    path: invalid.into(),
                    is_dir: false,
                    size: 0,
                    modified_ms: None,
                }]))?;
                Ok(1)
            });
            assert!(result.is_err());
            assert_eq!(index.status().unwrap().count, 0);
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

    #[test]
    fn first_batch_rechecks_revocation_and_preserves_existing_identity() {
        fn prepared(row: FileRecord, sweep: u64) -> Vec<Entry> {
            vec![Entry::new(
                Raw {
                    key: path_key_text(&row.path),
                    has_name: true,
                    path: row.path,
                    is_dir: row.is_dir,
                    size: row.size,
                    modified_ms: row.modified_ms,
                },
                sweep,
            )]
        }
        let (index, owner) = isolated();
        let dir = tempfile::tempdir().unwrap();
        let existing = record(dir.path(), "existing.md");
        index
            .collect_service_snapshot(1, dir.path(), 1, |_, receive| {
                receive(Event::Records(vec![existing.clone()]))?;
                Ok(1)
            })
            .unwrap();
        let original_key = index
            .catalog
            .read()
            .unwrap()
            .entries
            .keys()
            .next()
            .unwrap()
            .clone();
        // A row may appear after the initial-empty check but before this batch
        // obtains its write lock. Its allocation and revision must be preserved.
        index.publish_first_batch(1, prepared(existing, 2)).unwrap();
        {
            let catalog = index.catalog.read().unwrap();
            assert_eq!(catalog.status.revision, 1);
            assert!(Arc::ptr_eq(
                &original_key,
                catalog.entries.keys().next().unwrap()
            ));
            assert_eq!(catalog.entries.values().next().unwrap().seen, 2);
        }
        index
            .publish_first_batch(1, prepared(record(dir.path(), "new.md"), 3))
            .unwrap();
        assert_eq!(index.status().unwrap().count, 2);
        index.generation.store(2, Ordering::Release);
        assert_eq!(
            index
                .publish_first_batch(1, prepared(record(dir.path(), "revoked.md"), 4))
                .unwrap_err()
                .kind(),
            io::ErrorKind::ConnectionAborted
        );
        index.generation.store(1, Ordering::Release);
        drop(owner);
        assert_eq!(
            index
                .publish_first_batch(1, prepared(record(dir.path(), "owner-gone.md"), 5))
                .unwrap_err()
                .kind(),
            io::ErrorKind::ConnectionAborted
        );
        let catalog = index.catalog.read().unwrap();
        assert_eq!(catalog.entries.len(), 2);
        assert_eq!(catalog.status.revision, 2);
        assert!(catalog
            .entries
            .values()
            .all(|entry| !matches!(entry.name(), "revoked.md" | "owner-gone.md")));
    }

    #[test]
    fn late_snapshot_failure_never_publishes_private_batches() {
        for failure in 0..3 {
            let (index, _owner) = isolated();
            let dir = tempfile::tempdir().unwrap();
            index
                .collect_service_snapshot(1, dir.path(), 1, |_, receive| {
                    receive(Event::Records(vec![record(dir.path(), "existing.md")]))?;
                    Ok(1)
                })
                .unwrap();
            let result = index.collect_service_snapshot(1, dir.path(), 2, |_, receive| {
                // A large private reply must still insert nothing when its
                // final validation fails or the authorized root is revoked.
                for batch in 0..64 {
                    receive(Event::Records(
                        (0..32)
                            .map(|n| record(dir.path(), &format!("비공개-{batch}-{n}.md")))
                            .collect(),
                    ))?;
                    assert_eq!(index.status().unwrap().count, 1);
                }
                match failure {
                    0 => Err(io::Error::new(io::ErrorKind::UnexpectedEof, "partial")),
                    1 => Ok(0),
                    _ => {
                        index.generation.store(2, Ordering::Release);
                        Ok(1)
                    }
                }
            });
            assert!(result.is_err());
            let catalog = index.catalog.read().unwrap();
            assert_eq!(catalog.entries.len(), 1);
            assert_eq!(catalog.status.revision, 1);
            let entry = catalog.entries.values().next().unwrap();
            assert_eq!(entry.name(), "existing.md");
            assert_eq!(entry.seen, 1, "a rejected sweep cannot refresh old rows");
        }
    }

    #[test]
    fn streamed_paths_preserve_unicode_search_display_and_metadata_updates() {
        let (index, _owner) = isolated();
        let dir = tempfile::tempdir().unwrap();
        let mut rows = vec![
            record(dir.path(), "Report.MD"),
            record(dir.path(), "한글-😀.TXT"),
            record(dir.path(), "한글.TXT"),
        ];
        for sweep in [1, 2, 3] {
            if sweep == 3 {
                rows[0].size = 84;
                rows[0].modified_ms = Some(99);
            }
            index
                .collect_service_snapshot(1, dir.path(), sweep, |_, receive| {
                    receive(Event::Records(rows.clone()))?;
                    Ok(1)
                })
                .unwrap();
            assert_eq!(
                index.status().unwrap().revision,
                if sweep < 3 { 1 } else { 2 }
            );
        }
        let korean = index.search("한글", "txt", None, 10).unwrap();
        assert_eq!(korean.total, 2);
        assert!(korean.results.iter().any(|hit| hit.name == "한글-😀.TXT"));
        assert!(korean.results.iter().any(|hit| hit.name == "한글.TXT"));
        let english = index.search("report", "md", None, 10).unwrap();
        assert_eq!(english.total, 1);
        assert_eq!(english.results[0].name, "Report.MD");
        assert_eq!(english.results[0].size, 84);
        assert_eq!(english.results[0].modified_ms, Some(99));
    }
}

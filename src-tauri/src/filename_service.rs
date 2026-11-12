//! Optional snapshot source for complete local drives. The real catalog only
//! changes after the client has validated a reply's terminal frame and open
//! root identity. A first catalog may meanwhile be searched as a provisional
//! view (`Provisional` in `index.rs`), which any failure detaches.
//! Ordinary folder walks remain available on every OS and on service failure.
use super::*;
#[cfg(any(windows, test))]
use prism_ntfs_protocol::stream::Event;
#[cfg(any(windows, test))]
use std::io;
#[cfg(any(windows, test))]
use std::time::Instant;
#[cfg(any(windows, test))]
#[path = "filename_catalog.rs"]
mod catalog;
/// How often the window is told about more first-scan rows. The window
/// itself also limits re-searching to once per two seconds while scanning.
#[cfg(any(windows, test))]
const PROVISIONAL_REFRESH: Duration = Duration::from_secs(1);
#[cfg(any(windows, test))]
#[path = "filename_pipeline.rs"]
mod pipeline;

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
        let started = Instant::now();
        let mut first_batch = None;
        let mut received_records = 0usize;
        let mut receive_callback = Duration::ZERO;
        let mut validated_fetch = Duration::ZERO;
        let root_key = path_key(root);
        let drive = (root_key.len() == 2 && root_key.as_bytes()[1] == b':')
            .then(|| prism_ntfs_protocol::DriveLetter::new(root_key.as_bytes()[0]).ok())
            .flatten();
        let cancel = Arc::new(AtomicBool::new(false));
        let initial = self
            .catalog
            .read()
            .is_ok_and(|catalog| catalog.entries.is_empty());
        let threads = std::thread::available_parallelism()
            .map_or(2, usize::from)
            .clamp(1, 8);
        let mut rows = Vec::new();
        let mut row_count = 0usize;
        let mut shown = Instant::now();
        let prepared = std::thread::scope(|scope| {
            let (stop, stopped) = mpsc::channel::<()>();
            let cancel = &cancel;
            let mut preparation: Option<pipeline::Preparation<'_>> = None;
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
                        if !records.is_empty() {
                            first_batch.get_or_insert_with(|| started.elapsed());
                        }
                        let callback_started = Instant::now();
                        received_records = received_records.saturating_add(records.len());
                        for record in &records {
                            // Whole-drive validation needs no allocated path
                            // key. Only validated records may reach private
                            // workers; during transfer they are at most a
                            // provisional search view, never the real catalog.
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
                        if let Some(preparation) = &preparation {
                            preparation.send(records)?;
                            self.refresh_provisional(generation, preparation.shared(), &mut shown);
                        } else {
                            row_count = row_count.saturating_add(records.len());
                            rows.push(records);
                            // A single-core host also needs bounded raw rows;
                            // its one preparation worker drains a one-slot queue.
                            if initial && row_count >= 4096 {
                                preparation = Some(pipeline::Preparation::start(
                                    scope, self, generation, sweep, threads,
                                )?);
                                let preparation = preparation.as_ref().unwrap();
                                for batch in std::mem::take(&mut rows) {
                                    preparation.send(batch)?;
                                }
                                self.show_provisional(generation, preparation.shared());
                                shown = Instant::now();
                            }
                        }
                        receive_callback += callback_started.elapsed();
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
            validated_fetch = started.elapsed();
            drop(stop);
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
            // Dropping a failed fetch cancels and joins its private workers.
            // Only the successful terminal/root-validated path may merge them.
            preparation.map(pipeline::Preparation::join).transpose()
        });
        let published = match prepared {
            Ok(Some(shared)) => self.publish_provisional(generation, &shared),
            Ok(None) => self.publish_service_rows(generation, rows, sweep, initial, threads),
            Err(error) => Err(error),
        };
        if let Err(error) = published {
            // Failure, cancellation, an out-of-scope frame or a revoked owner:
            // searches stop showing this attempt's rows at once. The real
            // catalog was never touched, so nothing else needs undoing.
            self.drop_provisional(generation);
            return Err(error);
        }
        let total = started.elapsed();
        let millis = |duration: Duration| duration.as_millis().min(u64::MAX as u128) as u64;
        if let Ok(mut catalog) = self.catalog.write() {
            // Only the current, fully authenticated/root-validated collection
            // may replace the last successful diagnostic. No paths are kept.
            if self.current(generation) {
                catalog.status.last_service_snapshot = Some(ServiceSnapshotTiming {
                    received_records,
                    first_batch_ms: first_batch.map(millis),
                    validated_fetch_ms: millis(validated_fetch),
                    receive_callback_ms: millis(receive_callback),
                    finish_publish_ms: millis(total.saturating_sub(validated_fetch)),
                    total_ms: millis(total),
                    initial_catalog: initial,
                });
            }
        }
        Ok(())
    }

    /// Let searches read the first scan's private rows while they arrive.
    /// Only for a current generation whose real catalog is still empty.
    #[cfg(any(windows, test))]
    fn show_provisional(&self, generation: u64, shared: &Arc<Mutex<catalog::Prepared>>) {
        let Ok(mut catalog) = self.catalog.write() else {
            return;
        };
        if !self.current(generation) || !catalog.entries.is_empty() || catalog.provisional.is_some()
        {
            return;
        }
        catalog.provisional = Some(Provisional {
            generation,
            entries: shared.clone(),
        });
        catalog.status.count = shared.lock().unwrap_or_else(|e| e.into_inner()).len();
        catalog.status.revision += 1;
    }

    /// About once a second, publish the provisional row count and a new
    /// revision so the window searches again. Never waits for a running
    /// search: when the catalog is busy, the next frame tries again.
    #[cfg(any(windows, test))]
    fn refresh_provisional(
        &self,
        generation: u64,
        shared: &Arc<Mutex<catalog::Prepared>>,
        shown: &mut Instant,
    ) {
        if shown.elapsed() < PROVISIONAL_REFRESH {
            return;
        }
        let Ok(mut catalog) = self.catalog.try_write() else {
            return;
        };
        *shown = Instant::now();
        let ours = catalog.provisional.as_ref().is_some_and(|provisional| {
            provisional.generation == generation && Arc::ptr_eq(&provisional.entries, shared)
        });
        if ours && self.current(generation) {
            catalog.status.count = shared.lock().unwrap_or_else(|e| e.into_inner()).len();
            catalog.status.revision += 1;
        }
    }

    /// Swap the validated first-scan rows in, in one step under the catalog
    /// lock, so a search sees either the provisional or the real rows.
    #[cfg(any(windows, test))]
    fn publish_provisional(
        &self,
        generation: u64,
        shared: &Arc<Mutex<catalog::Prepared>>,
    ) -> io::Result<()> {
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
        let prepared = std::mem::take(&mut *shared.lock().unwrap_or_else(|e| e.into_inner()));
        if catalog
            .provisional
            .as_ref()
            .is_some_and(|provisional| Arc::ptr_eq(&provisional.entries, shared))
        {
            catalog.provisional = None;
        }
        // The usual case: the real catalog is still empty and takes the rows
        // whole, under this same lock (no moment with neither shown).
        catalog.status.revision += 1;
        if catalog.entries.is_empty() {
            let mut prepared = prepared;
            prepared.shrink_to_fit();
            catalog.entries = prepared;
            catalog.status.count = catalog.entries.len();
            catalog.status.scanning = true;
            return Ok(());
        }
        // Another valid writer filled the catalog meanwhile: keep its rows
        // and merge ours in checked batches, as before.
        catalog.status.count = catalog.entries.len();
        drop(catalog);
        self.publish_initial_catalog(generation, prepared)
    }

    /// Detach this generation's provisional rows (if still shown).
    #[cfg(any(windows, test))]
    fn drop_provisional(&self, generation: u64) {
        let Ok(mut catalog) = self.catalog.write() else {
            return;
        };
        if catalog
            .provisional
            .as_ref()
            .is_some_and(|provisional| provisional.generation == generation)
        {
            catalog.provisional = None;
            catalog.status.count = catalog.entries.len();
            catalog.status.revision += 1;
        }
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
        // its bounded batch outside the lock; an unchanged stored row only
        // gets its sweep id refreshed, anything else is stored or replaced.
        let mut changed = false;
        let mut path = String::new();
        for entry in entries {
            let unchanged = catalog.entries.get(&entry.key).and_then(|stored| {
                (stored.is_dir() == entry.is_dir
                    && stored.size() == entry.size
                    && stored.modified_ms() == entry.modified_ms
                    && stored.write_path(&mut path) == &*entry.path)
                    .then(|| stored.id())
            });
            match unchanged {
                Some(id) => catalog.entries.set_seen(id, entry.seen),
                None => {
                    catalog.entries.insert(&entry);
                    changed = true;
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
#[path = "filename_service_bench.rs"]
mod benchmark;

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
    fn snapshot_timings_remain_private_until_validation_and_reset_with_generation() {
        let (index, _owner) = isolated();
        let dir = tempfile::tempdir().unwrap();
        index
            .collect_service_snapshot(1, dir.path(), 1, |_, receive| {
                std::thread::sleep(Duration::from_millis(20));
                receive(Event::Records(vec![record(dir.path(), "private-name.md")]))?;
                assert!(index.status().unwrap().last_service_snapshot.is_none());
                std::thread::sleep(Duration::from_millis(20));
                Ok(1)
            })
            .unwrap();
        let status = index.status().unwrap();
        let timing = status.last_service_snapshot.as_ref().unwrap();
        assert_eq!(timing.received_records, 1);
        assert!(timing.initial_catalog);
        assert!(timing.first_batch_ms.unwrap() >= 10);
        assert!(timing.validated_fetch_ms >= timing.first_batch_ms.unwrap());
        assert!(timing.receive_callback_ms <= timing.validated_fetch_ms);
        assert!(timing.total_ms >= timing.validated_fetch_ms);
        assert!(timing.total_ms >= timing.finish_publish_ms);
        let public = serde_json::to_value(&status).unwrap();
        assert!(public.get("lastServiceSnapshot").is_none());
        assert_eq!(public.as_object().unwrap().len(), 7);
        let diagnostic = serde_json::to_value(timing).unwrap();
        assert_eq!(diagnostic["receivedRecords"], 1);
        assert!(!diagnostic.to_string().contains("private-name"));
        assert!(!diagnostic
            .to_string()
            .contains(dir.path().to_string_lossy().as_ref()));
        index.refresh(vec![]);
        assert!(index.status().unwrap().last_service_snapshot.is_none());
    }

    #[test]
    fn an_empty_validated_snapshot_has_no_first_record_batch() {
        let (index, _owner) = isolated();
        let dir = tempfile::tempdir().unwrap();
        index
            .collect_service_snapshot(1, dir.path(), 1, |_, _| Ok(1))
            .unwrap();
        let timing = index.status().unwrap().last_service_snapshot.unwrap();
        assert_eq!(timing.received_records, 0);
        assert_eq!(timing.first_batch_ms, None);
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
        let previous_timing = index.status().unwrap().last_service_snapshot;
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
                index.status().unwrap().last_service_snapshot,
                previous_timing
            );
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
        // obtains its write lock. Its stored entry and revision must be kept:
        // only the sweep id is refreshed, nothing is stored again.
        let insertions = index.catalog.read().unwrap().entries.insertions();
        index.publish_first_batch(1, prepared(existing, 2)).unwrap();
        {
            let catalog = index.catalog.read().unwrap();
            assert_eq!(catalog.status.revision, 1);
            assert_eq!(catalog.entries.insertions(), insertions);
            assert_eq!(&original_key, &catalog.entries.keys().next().unwrap());
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

    /// Search the provisional/real catalog for "private" rows, waiting
    /// briefly for the preparation workers that add them in the background.
    fn visible(index: &FileIndex, query: &str, at_least: usize) -> usize {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let total = index.search(query, "", None, 10).unwrap().total;
            if total >= at_least || Instant::now() > deadline {
                return total;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_first_catalog_is_searchable_while_streaming_and_detached_on_every_failure() {
        for failure in 0..5 {
            let (index, owner) = isolated();
            let mut owner = Some(owner);
            let root = tempfile::tempdir().unwrap();
            let outside = tempfile::tempdir().unwrap();
            let result = index.collect_service_snapshot(1, root.path(), 1, |_, receive| {
                for batch in 0..8 {
                    receive(Event::Records(
                        (0..1024)
                            .map(|number| {
                                record(root.path(), &format!("private-{batch}-{number}.md"))
                            })
                            .collect(),
                    ))?;
                }
                // Before the terminal frame: rows are searchable, the real
                // catalog stays empty.
                assert!(visible(&index, "private", 1) > 0);
                assert!(index.catalog.read().unwrap().entries.is_empty());
                assert!(index.status().unwrap().count > 0);
                match failure {
                    0 => Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "incomplete fixture",
                    )),
                    1 => Ok(0),
                    2 => {
                        index.generation.store(2, Ordering::Release);
                        Ok(1)
                    }
                    3 => {
                        receive(Event::Records(vec![record(outside.path(), "outside.md")]))?;
                        Ok(1)
                    }
                    _ => {
                        drop(owner.take());
                        Ok(1)
                    }
                }
            });
            assert!(result.is_err());
            // Every failure detaches the provisional rows: nothing is left to
            // search, count or open, and nothing reached the real catalog.
            let catalog = index.catalog.read().unwrap();
            assert!(catalog.provisional.is_none(), "failure {failure}");
            assert!(catalog.entries.is_empty());
            assert_eq!(catalog.status.count, 0);
            drop(catalog);
            assert_eq!(index.search("private", "", None, 10).unwrap().total, 0);
        }
    }

    #[test]
    fn a_validated_first_catalog_replaces_its_provisional_rows_in_one_step() {
        let (index, _owner) = isolated();
        let root = tempfile::tempdir().unwrap();
        // A real file, so opening a provisional result can be checked too.
        fs::write(root.path().join("private-0-0.md"), "x").unwrap();
        let canonical = fs::canonicalize(root.path()).unwrap();
        index.catalog.write().unwrap().roots.push(Root {
            id: "root".into(),
            key: path_key(&canonical),
            path: canonical.clone(),
            source: root.path().to_string_lossy().into_owned(),
            available: true,
        });
        let opened = canonical.join("private-0-0.md");
        let mut revision_while_streaming = 0;
        index
            .collect_service_snapshot(1, &canonical, 1, |_, receive| {
                for batch in 0..8 {
                    receive(Event::Records(
                        (0..1024)
                            .map(|number| {
                                record(&canonical, &format!("private-{batch}-{number}.md"))
                            })
                            .collect(),
                    ))?;
                }
                assert!(visible(&index, "private", 8 * 1024) == 8 * 1024);
                // Scoped searches and opening a shown result work as well.
                let scoped = index.search("private", "", Some("root"), 10).unwrap();
                assert_eq!(scoped.total, 8 * 1024);
                index
                    .resolve_result(&opened.to_string_lossy())
                    .expect("a provisional result opens");
                revision_while_streaming = index.status().unwrap().revision;
                assert!(revision_while_streaming > 0, "the window is told to search");
                Ok(1)
            })
            .unwrap();
        let catalog = index.catalog.read().unwrap();
        assert!(catalog.provisional.is_none());
        assert_eq!(catalog.entries.len(), 8 * 1024);
        assert_eq!(catalog.status.count, 8 * 1024);
        assert!(catalog.status.revision > revision_while_streaming);
        drop(catalog);
        assert_eq!(
            index.search("private", "", None, 10).unwrap().total,
            8 * 1024
        );
    }

    #[test]
    fn a_new_generation_hides_and_detaches_an_unfinished_first_scan() {
        let (index, _owner) = isolated();
        let root = tempfile::tempdir().unwrap();
        let result = index.collect_service_snapshot(1, root.path(), 1, |_, receive| {
            receive(Event::Records(
                (0..5000)
                    .map(|number| record(root.path(), &format!("private-{number}.md")))
                    .collect(),
            ))?;
            assert!(visible(&index, "private", 1) > 0);
            // A scope change (refresh/configure/reset) starts a new generation.
            index.refresh(vec![]);
            assert!(index.catalog.read().unwrap().provisional.is_none());
            assert_eq!(index.search("private", "", None, 10).unwrap().total, 0);
            Ok(1)
        });
        assert!(result.is_err());
        assert!(index.catalog.read().unwrap().entries.is_empty());
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

use super::*;

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
fn rows(root: &Path, count: usize) -> Vec<FileRecord> {
    (0..count)
        .map(|number| FileRecord {
            path: root
                .join(format!(
                    "directory-{}/{}-{number}.{}",
                    number % 19,
                    if number % 3 == 0 {
                        "한글-日本語-😀"
                    } else {
                        "Report"
                    },
                    if number % 3 == 0 { "TXT" } else { "MD" }
                ))
                .to_string_lossy()
                .into_owned(),
            is_dir: number % 101 == 0,
            size: number as u64 * 13,
            modified_ms: Some(1609459200000 + number as i64),
        })
        .collect()
}
fn batches(rows: Vec<FileRecord>) -> Vec<Vec<FileRecord>> {
    let mut input = rows.into_iter();
    let mut result = Vec::new();
    loop {
        let next: Vec<_> = input.by_ref().take(1024).collect();
        if next.is_empty() {
            return result;
        }
        result.push(next);
    }
}
fn same(actual: &Entry, expected: &Entry) {
    assert_eq!(actual.path, expected.path);
    assert_eq!(actual.key, expected.key);
    assert_eq!(actual.search, expected.search);
    assert_eq!(actual.name_start, expected.name_start);
    assert_eq!(actual.initials_path, expected.initials_path);
    assert_eq!(actual.keyboard_path, expected.keyboard_path);
    assert_eq!(actual.is_dir, expected.is_dir);
    assert_eq!(actual.size, expected.size);
    assert_eq!(actual.modified_ms, expected.modified_ms);
    assert_eq!(actual.seen, expected.seen);
}

#[test]
fn private_trees_preserve_every_unicode_projection_and_metadata_field() {
    let root = tempfile::tempdir().unwrap();
    let input = rows(root.path(), 8192);
    let mut expected = Prepared::new();
    for record in input.clone() {
        insert(&mut expected, Entry::new(raw(record), 9));
    }
    for workers in [1, 2, 3, 4, 5, 8] {
        let (index, _owner) = isolated();
        let actual = index
            .prepare_initial_catalog(1, batches(input.clone()), 9, workers)
            .unwrap();
        assert_eq!(
            index.status().unwrap().count,
            0,
            "preparation stays private"
        );
        assert_eq!(actual.len(), expected.len());
        for (entry, expected_entry) in actual.values().zip(expected.values()) {
            same(&entry, &expected_entry);
        }
    }
}

#[test]
fn one_complete_publication_remains_searchable_and_unchanged_sweeps_reuse_entries() {
    let root = tempfile::tempdir().unwrap();
    let input = rows(root.path(), 8192);
    let (index, _owner) = isolated();
    let prepared = index
        .prepare_initial_catalog(1, batches(input.clone()), 1, 4)
        .unwrap();
    index.publish_initial_catalog(1, prepared).unwrap();
    let status = index.status().unwrap();
    assert_eq!(
        (status.count, status.revision, status.scanning),
        (8192, 1, true)
    );
    assert!(index.search("한글", "txt", None, 20).unwrap().total > 1000);
    assert!(index.search("report", "md", None, 20).unwrap().total > 1000);
    let original = index.catalog.read().unwrap().entries.insertions();
    let prepared = index
        .prepare_initial_catalog(1, batches(input), 2, 4)
        .unwrap();
    index.publish_initial_catalog(1, prepared).unwrap();
    let catalog = index.catalog.read().unwrap();
    assert_eq!(catalog.status.revision, 1);
    // Unchanged rows only get their sweep id refreshed; nothing is stored again.
    assert_eq!(catalog.entries.insertions(), original);
    assert!(catalog.entries.values().all(|entry| entry.seen == 2));
}

#[test]
fn publication_rechecks_generation_and_owner_before_exposing_prepared_rows() {
    let root = tempfile::tempdir().unwrap();
    for owner_gone in [false, true] {
        let (index, owner) = isolated();
        let prepared = index
            .prepare_initial_catalog(1, batches(rows(root.path(), 1024)), 1, 4)
            .unwrap();
        if owner_gone {
            drop(owner);
        } else {
            index.generation.fetch_add(1, Ordering::SeqCst);
        }
        assert_eq!(
            index
                .publish_initial_catalog(1, prepared)
                .unwrap_err()
                .kind(),
            io::ErrorKind::ConnectionAborted
        );
        assert_eq!(index.status().unwrap().count, 0);
    }
    let (index, _owner) = isolated();
    index.generation.store(2, Ordering::SeqCst);
    assert_eq!(
        index
            .prepare_initial_catalog(1, batches(rows(root.path(), 16)), 1, 4)
            .err()
            .unwrap()
            .kind(),
        io::ErrorKind::ConnectionAborted
    );
    for workers in [0, 9] {
        assert_eq!(
            index
                .prepare_initial_catalog(2, Vec::new(), 1, workers)
                .err()
                .unwrap()
                .kind(),
            io::ErrorKind::InvalidInput
        );
    }
}

#[test]
fn racing_nonempty_catalog_preserves_other_entries_and_unchanged_stored_rows() {
    let root = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let input = rows(root.path(), 1024);
    let (index, _owner) = isolated();
    let prepared = index
        .prepare_initial_catalog(1, batches(input.clone()), 2, 4)
        .unwrap();
    let existing = Entry::new(raw(input[0].clone()), 1);
    let original_key = existing.key.clone();
    let other = Entry::new(raw(rows(elsewhere.path(), 1).pop().unwrap()), 1);
    let other_key = other.key.clone();
    index.publish_first_batch(1, vec![existing, other]).unwrap();
    let revision = index.status().unwrap().revision;
    index.publish_initial_catalog(1, prepared).unwrap();
    let catalog = index.catalog.read().unwrap();
    assert_eq!(catalog.entries.len(), 1025);
    // The unchanged racing row keeps its stored entry (refreshed sweep id);
    // the other writer's row stays untouched.
    let seen = |key: &str| catalog.entries.get(key).unwrap().seen();
    assert_eq!(seen(&other_key), 1);
    assert_eq!(seen(&original_key), 2);
    assert!(catalog.status.revision >= revision);
}

#[test]
fn repeated_identities_keep_complete_metadata_after_merging() {
    let root = tempfile::tempdir().unwrap();
    let original = rows(root.path(), 32);
    let mut repeated = Vec::new();
    for _ in 0..160 {
        repeated.extend(original.clone());
    }
    let (index, _owner) = isolated();
    let prepared = index
        .prepare_initial_catalog(1, batches(repeated), 7, 8)
        .unwrap();
    assert_eq!(prepared.len(), original.len());
    for entry in prepared.values() {
        let record = original
            .iter()
            .find(|record| path_key_text(&record.path) == entry.key.as_ref())
            .unwrap();
        same(&entry, &Entry::new(raw(record.clone()), 7));
    }
}

#[test]
fn failed_worker_is_joined_with_its_still_running_siblings() {
    let stop = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let (release, waiting) = mpsc::channel::<()>();
        let (done, finished) = mpsc::channel();
        let blocked = scope.spawn(move || {
            waiting.recv().map_err(|_| worker_error())?;
            Ok(Prepared::new())
        });
        let failed = scope.spawn(|| Err(worker_error()));
        let stop_ref = &stop;
        let joiner = scope.spawn(move || {
            let result = join_all(vec![failed, blocked], stop_ref);
            done.send(result.is_err()).unwrap();
        });
        let start = std::time::Instant::now();
        while !stop.load(Ordering::Acquire) && start.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(
            stop.load(Ordering::Acquire),
            "first failure must stop the remaining workers"
        );
        assert_eq!(
            finished.recv_timeout(Duration::from_millis(50)),
            Err(mpsc::RecvTimeoutError::Timeout),
            "real sibling still owns work"
        );
        release.send(()).unwrap();
        assert!(finished.recv_timeout(Duration::from_secs(2)).unwrap());
        joiner.join().unwrap();
    });
}

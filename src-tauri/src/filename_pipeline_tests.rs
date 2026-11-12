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
fn rows(root: &Path, first: usize, count: usize) -> Vec<FileRecord> {
    (first..first + count)
        .map(|number| FileRecord {
            path: root
                .join(format!("한글-日本語-Report-😀-{number}.MD"))
                .to_string_lossy()
                .into_owned(),
            is_dir: number % 97 == 0,
            size: number as u64 * 13,
            modified_ms: Some(number as i64 - 1609459200000),
        })
        .collect()
}
fn wait(check: impl Fn() -> bool) {
    let start = std::time::Instant::now();
    while !check() && start.elapsed() < Duration::from_secs(3) {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(check(), "owned fixture worker did not reach its checkpoint");
}

#[test]
fn real_workers_prepare_before_the_producer_finishes_and_keep_all_fields_private() {
    for workers in [1, 4] {
        prepare_before_producer_finishes(workers);
    }
}

fn prepare_before_producer_finishes(workers: usize) {
    let root = tempfile::tempdir().unwrap();
    let input = rows(root.path(), 0, 8192);
    let (index, _owner) = isolated();
    let expected = index
        .prepare_initial_catalog(1, vec![input.clone()], 9, 4)
        .unwrap();
    let prepared = std::thread::scope(|scope| {
        let preparation = Preparation::start(scope, &index, 1, 9, workers).unwrap();
        // Inject a batch larger than a wire frame to also exercise bounded
        // splitting; the production wire decoder already caps each frame.
        preparation.send(input[..4096].to_vec()).unwrap();
        wait(|| preparation.processed.load(Ordering::Acquire) == 4096);
        assert_eq!(index.status().unwrap().count, 0);
        assert_eq!(Arc::strong_count(&index.lifetime), 2);
        preparation.send(input[4096..].to_vec()).unwrap();
        preparation.finish().unwrap()
    });
    assert_eq!(index.status().unwrap().revision, 0);
    assert_eq!(prepared.len(), expected.len());
    for ((key, actual), (expected_key, expected)) in prepared.iter().zip(&expected) {
        assert_eq!(key, expected_key);
        assert!(Arc::ptr_eq(key, &actual.key));
        assert_eq!(actual.path, expected.path);
        assert_eq!(actual.search, expected.search);
        assert_eq!(actual.initials_path, expected.initials_path);
        assert_eq!(actual.keyboard_path, expected.keyboard_path);
        assert_eq!(actual.name_start, expected.name_start);
        assert_eq!(actual.is_dir, expected.is_dir);
        assert_eq!(actual.size, expected.size);
        assert_eq!(actual.modified_ms, expected.modified_ms);
        assert_eq!(actual.seen, expected.seen);
    }
    index.publish_initial_catalog(1, prepared).unwrap();
    assert_eq!(index.status().unwrap().count, 8192);
}

#[test]
fn full_bounded_queue_still_notices_generation_and_owner_revocation() {
    for revoke_owner in [false, true] {
        let (index, owner) = isolated();
        let root = tempfile::tempdir().unwrap();
        std::thread::scope(|scope| {
            let (sender, _held_receiver) = mpsc::sync_channel(2);
            let preparation = Preparation {
                index: &index,
                generation: 1,
                sender: Some(sender),
                workers: vec![],
                stop: Arc::new(AtomicBool::new(false)),
                processed: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            };
            preparation.send(rows(root.path(), 0, 1)).unwrap();
            preparation.send(rows(root.path(), 1, 1)).unwrap();
            let (sent, result) = mpsc::channel();
            let index_ref = &index;
            scope.spawn(move || {
                let error = preparation.send(rows(root.path(), 2, 1)).unwrap_err();
                sent.send(error.kind()).unwrap();
            });
            assert_eq!(
                result.recv_timeout(Duration::from_millis(50)),
                Err(mpsc::RecvTimeoutError::Timeout),
                "the third batch must remain behind two occupied slots"
            );
            if revoke_owner {
                drop(owner);
            } else {
                index_ref.generation.store(2, Ordering::Release);
            }
            assert_eq!(
                result.recv_timeout(Duration::from_secs(2)).unwrap(),
                io::ErrorKind::ConnectionAborted
            );
        });
        assert_eq!(index.status().unwrap().count, 0);
    }
}

#[test]
fn producer_failure_or_panic_closes_the_queue_and_joins_the_actual_workers() {
    for panic in [false, true] {
        let (index, _owner) = isolated();
        let root = tempfile::tempdir().unwrap();
        let mut progress = None;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            std::thread::scope(|scope| {
                let preparation = Preparation::start(scope, &index, 1, 3, 4).unwrap();
                progress = Some(preparation.processed.clone());
                preparation.send(rows(root.path(), 0, 4096)).unwrap();
                wait(|| preparation.processed.load(Ordering::Acquire) > 0);
                if panic {
                    panic!("owned producer fixture failed");
                }
                drop(preparation);
            });
        }));
        assert_eq!(result.is_err(), panic);
        assert_eq!(Arc::strong_count(progress.as_ref().unwrap()), 1);
        assert_eq!(Arc::strong_count(&index.lifetime), 2);
        assert_eq!(index.status().unwrap().count, 0);
    }
}

#[test]
fn a_completed_stream_keeps_unchanged_allocations_on_the_next_sweep() {
    let (index, _owner) = isolated();
    let root = tempfile::tempdir().unwrap();
    let input = rows(root.path(), 0, 8192);
    let mut original = None;
    let mut revision = 0;
    for sweep in [1, 2] {
        index
            .collect_service_snapshot(1, root.path(), sweep, |_, receive| {
                for batch in input.chunks(MAX_BATCH_RECORDS) {
                    receive(prism_ntfs_protocol::stream::Event::Records(batch.to_vec()))?;
                }
                Ok(sweep)
            })
            .unwrap();
        let catalog = index.catalog.read().unwrap();
        assert_eq!(catalog.entries.len(), input.len());
        let first = catalog.entries.first_key_value().unwrap();
        if let Some(original) = &original {
            assert!(Arc::ptr_eq(first.0, original));
            assert_eq!(catalog.status.revision, revision);
        } else {
            original = Some(first.0.clone());
            revision = catalog.status.revision;
        }
        assert!(catalog.entries.values().all(|entry| entry.seen == sweep));
    }
}

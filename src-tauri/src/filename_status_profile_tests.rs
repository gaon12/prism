use super::*;

#[test]
fn disabled_helpers_sample_no_clocks() {
    let catalog = RwLock::new(0);
    let prepared = Mutex::new(0);
    let before = CLOCK_READS.with(|reads| reads.get());
    assert!(Read::start(None, None).is_none());
    assert!(Read::start_command(None).is_none());
    assert!(Prune::start(false, 3, 7, 11).is_none());
    record_prune(None);
    *write_for(&catalog, None, WriterKind::Refresh).unwrap() = 1;
    *prepared_for(&prepared, None).unwrap() = 2;
    let reader = catalog.read().unwrap();
    assert!(matches!(
        try_write_for(&catalog, None, WriterKind::ProvisionalRefresh),
        Err(TryLockError::WouldBlock)
    ));
    drop(reader);
    assert_eq!(CLOCK_READS.with(|reads| reads.get()), before);
}

#[test]
fn prune_totals_saturate_and_slowest_keeps_one_coherent_record() {
    let mut summary = PruneSummary::default();
    summary.add(PruneRecord {
        roots: u64::MAX,
        node_slots: 21,
        directory_slots: 8,
        stale_candidates: 4,
        removed: 3,
        compactions: 1,
        deletion_ns: 50,
        compact_ns: 30,
        total_ns: 100,
        ..PruneRecord::default()
    });
    summary.add(PruneRecord {
        roots: 2,
        node_slots: 15,
        total_ns: 90,
        ..PruneRecord::default()
    });
    let json = serde_json::to_value(summary).unwrap();
    assert_eq!(json["calls"], 2);
    assert_eq!(json["totals"]["roots"], u64::MAX);
    assert_eq!(json["totals"]["nodeSlots"], 36);
    assert_eq!(json["slowest"]["nodeSlots"], 21);
    assert_eq!(json["slowest"]["compactNs"], 30);
    assert_eq!(json["slowest"]["deletionNs"], 50);
    assert_eq!(json["slowest"]["removed"], 3);
}

#[test]
fn read_snapshot_keeps_one_slowest_command_and_overlapping_regions_separate() {
    let collector = Collector::default();
    collector.read(ReadRecord {
        pool_wait_ns: Some(7),
        catalog_wait_ns: 11,
        catalog_hold_ns: 41,
        prepared_wait_ns: Some(13),
        prepared_hold_ns: Some(17),
        dto_build_ns: 5,
        worker_total_ns: 59,
        command_total_ns: Some(66),
        failed: false,
        snapshot_wait_ns: None,
        snapshot_hold_ns: None,
    });
    collector.read(ReadRecord {
        worker_total_ns: 3,
        failed: true,
        ..ReadRecord::default()
    });
    let snapshot = serde_json::to_value(collector.snapshot().unwrap()).unwrap();
    assert_eq!(snapshot["schemaVersion"], 1);
    assert_eq!(snapshot["readCalls"], 2);
    assert_eq!(snapshot["commandCalls"], 1);
    assert_eq!(snapshot["queuedCalls"], 1);
    assert_eq!(snapshot["readFailures"], 1);
    assert_eq!(snapshot["reads"][0]["calls"], 1);
    assert_eq!(snapshot["reads"][0]["totalNs"], 7);
    assert_eq!(snapshot["reads"][1]["calls"], 2);
    assert_eq!(snapshot["reads"][3]["calls"], 1);
    assert_eq!(snapshot["slowestCommand"]["commandTotalNs"], 66);
    assert_eq!(snapshot["slowestCommand"]["preparedWaitNs"], 13);
    assert_eq!(snapshot["overlappingDurationsDoNotSum"], true);
    assert_eq!(snapshot["holdEndsBeforeUnlock"], true);
    assert_eq!(snapshot["reads"].as_array().unwrap().len(), 10);
    assert_eq!(snapshot["writers"].as_array().unwrap().len(), 13);
}

#[test]
fn direct_command_records_its_span_without_inventing_a_pool_wait() {
    let collector = Collector::default();
    Read::start_command(Some(&collector)).unwrap().finish(false);
    let snapshot = serde_json::to_value(collector.snapshot().unwrap()).unwrap();
    assert_eq!(snapshot["readCalls"], 1);
    assert_eq!(snapshot["commandCalls"], 1);
    assert_eq!(snapshot["queuedCalls"], 0);
    assert_eq!(snapshot["reads"][0]["calls"], 0);
    assert!(snapshot["slowestCommand"]["poolWaitNs"].is_null());
    assert_eq!(
        snapshot["slowestCommand"]["commandTotalNs"],
        snapshot["slowestCommand"]["workerTotalNs"]
    );
}

#[test]
fn writer_unlocks_data_before_waiting_for_the_recorder() {
    let data = RwLock::new(0);
    let collector = Collector::default();
    let recorder = collector.state.lock().unwrap();
    std::thread::scope(|scope| {
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let data = &data;
        let collector = &collector;
        let worker = scope.spawn(move || {
            let mut guard = write_for(data, Some(collector), WriterKind::WalkPublish).unwrap();
            *guard = 7;
            ready_tx.send(()).unwrap();
            drop(guard);
        });
        ready_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        let started = Instant::now();
        loop {
            if let Ok(guard) = data.try_read() {
                assert_eq!(*guard, 7);
                break;
            }
            assert!(started.elapsed() < Duration::from_secs(3));
            std::thread::sleep(Duration::from_millis(1));
        }
        // Worker is blocked on this recorder, while the data is already free.
        drop(recorder);
        worker.join().unwrap();
    });
    let state = *collector.state.lock().unwrap();
    assert_eq!(
        state.writers[WriterKind::WalkPublish as usize].hold.calls,
        1
    );
}

#[test]
fn try_writer_preserves_would_block_without_a_hold_sample() {
    let collector = Collector::default();
    let data = RwLock::new(0);
    let _reader = data.read().unwrap();
    assert!(matches!(
        try_write_for(&data, Some(&collector), WriterKind::ProvisionalRefresh),
        Err(TryLockError::WouldBlock)
    ));
    let state = *collector.state.lock().unwrap();
    let sample = state.writers[WriterKind::ProvisionalRefresh as usize];
    assert_eq!(sample.attempts, 1);
    assert_eq!(sample.try_calls, 1);
    assert_eq!(sample.would_block, 1);
    assert_eq!(sample.hold.calls, 0);
}

#[test]
fn profiled_writer_preserves_poison_and_recovery() {
    let collector = Collector::default();
    let data = RwLock::new(0);
    std::thread::scope(|scope| {
        assert!(scope
            .spawn(|| {
                let _guard = write_for(&data, Some(&collector), WriterKind::Refresh).unwrap();
                panic!("owned poison fixture");
            })
            .join()
            .is_err());
    });
    assert!(data.is_poisoned());
    match write_for(&data, Some(&collector), WriterKind::Refresh) {
        Ok(_) => panic!("poison must remain observable"),
        Err(error) => *error.into_inner() = 2,
    }
    let state = *collector.state.lock().unwrap();
    let sample = state.writers[WriterKind::Refresh as usize];
    assert_eq!(sample.acquired, 2);
    assert_eq!(sample.poisoned, 1);
    assert_eq!(sample.hold.calls, 2);
    assert!(collector.snapshot().is_some());
}

fn isolated() -> (super::super::FileIndex, super::super::FileIndex) {
    use super::super::{Catalog, FileIndex};
    use std::sync::Arc;
    let (tx, _) = std::sync::mpsc::channel();
    let index = FileIndex {
        published: Arc::new(super::super::status::Store::default()),
        catalog: Arc::new(RwLock::new(Catalog {
            generation: 1,
            ..Default::default()
        })),
        generation: Arc::new(AtomicU64::new(1)),
        tx,
        lifetime: Arc::new(()),
    };
    (index.clone(), index)
}

#[test]
fn actual_status_hooks_follow_the_cached_gate_without_dto_or_private_text_changes() {
    use super::super::{Entries, Provisional};
    use std::sync::Arc;
    let (index, _owner) = isolated();
    {
        let mut catalog = index.write_catalog(WriterKind::Metadata).unwrap();
        catalog.status.warnings = vec!["private-path-and-query-canary".into()];
        catalog.provisional = Some(Provisional {
            progress: Arc::new(super::super::status::Progress::default()),
            generation: 1,
            entries: Arc::new(Mutex::new(Entries::new())),
        });
    }
    let prior_calls = snapshot().map_or(0, |s| s.read_calls);
    let before = CLOCK_READS.with(|reads| reads.get());
    let status = index.status_queued(queued()).unwrap();
    let command_status = index.status_for_command().unwrap();
    assert_eq!(command_status.count, status.count);
    assert_eq!(command_status.warnings, status.warnings);
    assert_eq!(status.count, 0);
    assert_eq!(status.warnings, ["private-path-and-query-canary"]);
    assert!(serde_json::to_value(status)
        .unwrap()
        .get("filenameStatusProfile")
        .is_none());
    if super::super::cpu_profile::enabled() {
        assert!(CLOCK_READS.with(|reads| reads.get()) > before);
        let profile = snapshot().unwrap();
        assert!(profile.read_calls > prior_calls);
        assert!(profile.queued_calls > 0);
        assert!(profile.command_calls > profile.queued_calls);
        assert!(!serde_json::to_string(&profile)
            .unwrap()
            .contains("private-path-and-query-canary"));
    } else {
        assert_eq!(CLOCK_READS.with(|reads| reads.get()), before);
        assert!(snapshot().is_none());
    }
}

#[test]
fn poisoned_actual_catalog_remains_an_error() {
    let (index, _owner) = isolated();
    std::thread::scope(|scope| {
        assert!(scope
            .spawn(|| {
                let _guard = index.write_catalog(WriterKind::Metadata).unwrap();
                panic!("owned status poison fixture");
            })
            .join()
            .is_err());
    });
    assert!(index.status().is_err());
    assert!(index.status_for_command().is_err());
}

#[test]
fn actual_status_releases_snapshot_lock_before_recording() {
    let (index, _owner) = isolated();
    let recorder = collector().map(|collector| collector.state.lock().unwrap());
    std::thread::scope(|scope| {
        let (result_tx, result_rx) = std::sync::mpsc::channel();
        let worker_index = index.clone();
        let worker = scope.spawn(move || {
            result_tx.send(worker_index.status().unwrap()).unwrap();
        });
        // The main thread can read the same snapshot even while recording is
        // blocked: snapshot acquisition never depends on the recorder mutex.
        let mut no_profile = None;
        assert_eq!(
            index.published.read(&index, &mut no_profile).unwrap().count,
            0
        );
        drop(recorder);
        assert_eq!(
            result_rx
                .recv_timeout(Duration::from_secs(3))
                .unwrap()
                .count,
            0
        );
        worker.join().unwrap();
    });
}

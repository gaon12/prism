use super::*;
use crate::index::{Catalog, Entries, Provisional};
use std::sync::{atomic::AtomicU64, mpsc, RwLock};
use std::time::Duration;

fn isolated() -> (FileIndex, FileIndex) {
    let (tx, _) = mpsc::channel();
    let index = FileIndex {
        published: Arc::new(Store::default()),
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
fn attach(index: &FileIndex, progress: &Arc<Progress>) -> Arc<Mutex<Entries>> {
    let entries = Arc::new(Mutex::new(Entries::new()));
    index
        .write_catalog(status_profile::WriterKind::ProvisionalShow)
        .unwrap()
        .provisional = Some(Provisional {
        generation: 1,
        entries: entries.clone(),
        progress: progress.clone(),
    });
    entries
}

#[test]
fn status_returns_while_catalog_and_prepared_writers_remain_held() {
    let (index, _owner) = isolated();
    let progress = Arc::new(Progress::default());
    let shared = attach(&index, &progress);
    progress.inserted(42);
    let catalog = index.catalog.write().unwrap();
    let prepared = shared.lock().unwrap();
    std::thread::scope(|scope| {
        let (tx, rx) = mpsc::channel();
        let index = &index;
        let worker = scope.spawn(move || {
            tx.send(index.status().unwrap()).unwrap();
        });
        // Completion before releasing either lock is the correctness property;
        // the timeout only bounds a regression instead of asserting small latency.
        let result = rx.recv_timeout(Duration::from_secs(3));
        drop(prepared);
        drop(catalog);
        assert_eq!(result.unwrap().count, 42);
        worker.join().unwrap();
    });
}

#[test]
fn late_and_out_of_order_batches_only_affect_their_exact_active_attempt() {
    let (index, _owner) = isolated();
    let first = Arc::new(Progress::default());
    attach(&index, &first);
    first.inserted(20);
    first.inserted(10);
    assert_eq!(index.status().unwrap().count, 20);
    index
        .write_catalog(status_profile::WriterKind::ProvisionalDrop)
        .unwrap()
        .provisional = None;
    first.inserted(500);
    assert_eq!(index.status().unwrap().count, 0);
    let second = Arc::new(Progress::default());
    attach(&index, &second);
    second.inserted(7);
    first.inserted(900);
    assert_eq!(index.status().unwrap().count, 7);
    // A metadata commit cannot overwrite newer progress with its old count.
    {
        let mut catalog = index
            .write_catalog(status_profile::WriterKind::Metadata)
            .unwrap();
        catalog.status.count = 2;
        catalog.status.revision = 13;
        catalog.status.scanning = true;
    }
    let status = index.status().unwrap();
    assert_eq!(
        (status.count, status.revision, status.scanning),
        (7, 13, true)
    );
}

#[test]
fn generation_revocation_hides_progress_before_catalog_can_be_reacquired() {
    let (index, _owner) = isolated();
    let progress = Arc::new(Progress::default());
    let shared = attach(&index, &progress);
    progress.inserted(42);
    let mut writer = index
        .write_catalog(status_profile::WriterKind::Metadata)
        .unwrap();
    let _prepared = shared.lock().unwrap();
    index.generation.store(2, Ordering::SeqCst);
    assert_eq!(index.status().unwrap().count, 0);
    progress.inserted(99);
    writer.status.revision = 99;
    drop(writer); // Revoked generation must not publish this late status.
    assert_eq!(index.status().unwrap().revision, 0);
    let mut catalog = index
        .write_catalog(status_profile::WriterKind::Refresh)
        .unwrap();
    catalog.generation = 2;
    catalog.provisional = None;
    catalog.status.revision = 1;
    drop(catalog);
    progress.inserted(1000);
    assert_eq!(
        (
            index.status().unwrap().revision,
            index.status().unwrap().count
        ),
        (1, 0)
    );
}

#[test]
fn owner_loss_hides_unvalidated_progress_without_waiting_for_a_worker() {
    let (index, owner) = isolated();
    let progress = Arc::new(Progress::default());
    attach(&index, &progress);
    progress.inserted(9);
    drop(owner);
    assert_eq!(index.status().unwrap().count, 0);
}

#[test]
fn live_catalog_replacement_stops_progress_and_releases_row_storage() {
    let (index, _owner) = isolated();
    let progress = Arc::new(Progress::default());
    let shared = attach(&index, &progress);
    let weak = Arc::downgrade(&shared);
    progress.inserted(9);
    {
        let mut catalog = index
            .write_catalog(status_profile::WriterKind::ProvisionalSwap)
            .unwrap();
        catalog.provisional = None;
        catalog.status.count = 3;
        catalog.status.initial_scan_complete = true;
        catalog.status.revision = 1;
    }
    drop(shared);
    assert!(
        weak.upgrade().is_none(),
        "progress store must never retain an arena"
    );
    progress.inserted(999);
    let status = index.status().unwrap();
    assert_eq!(
        (status.count, status.revision, status.initial_scan_complete),
        (3, 1, true)
    );
    // Live removals decrease counts; only an active provisional token uses max.
    index
        .write_catalog(status_profile::WriterKind::WatchPrune)
        .unwrap()
        .status
        .count = 1;
    assert_eq!(index.status().unwrap().count, 1);
}

#[test]
fn concurrent_status_readers_never_mix_committed_revision_and_flags() {
    let (index, _owner) = isolated();
    std::thread::scope(|scope| {
        let index = &index;
        let worker = scope.spawn(move || {
            for revision in 1..1000 {
                let mut catalog = index
                    .write_catalog(status_profile::WriterKind::Metadata)
                    .unwrap();
                catalog.status.revision = revision;
                catalog.status.count = revision as usize;
                catalog.status.scanning = revision % 2 == 1;
                catalog.status.initial_scan_complete = revision % 2 == 0;
            }
        });
        for _ in 0..1000 {
            let status = index.status().unwrap();
            assert_eq!(status.count as u64, status.revision);
            if status.revision != 0 {
                assert_eq!(status.scanning, status.revision % 2 == 1);
                assert_eq!(status.initial_scan_complete, status.revision % 2 == 0);
            }
        }
        worker.join().unwrap();
    });
}

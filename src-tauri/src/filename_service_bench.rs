//! Explicit synthetic client-only comparison. No pipe, live volume or model.
use super::*;
use prism_ntfs_protocol::FileRecord;
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    time::Instant,
};

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
fn rows(count: usize) -> Vec<Vec<FileRecord>> {
    let mut batches = Vec::new();
    let mut batch = Vec::new();
    for number in 0..count {
        batch.push(row(number));
        if batch.len() == 2048 {
            batches.push(std::mem::take(&mut batch));
        }
    }
    if !batch.is_empty() {
        batches.push(batch);
    }
    batches
}
fn row(number: usize) -> FileRecord {
    let directory = number.wrapping_mul(7919) % 10007;
    let label = if number.is_multiple_of(16) {
        "자료-한글-日本語"
    } else {
        "public-source"
    };
    FileRecord {
        path: format!(
            r"C:\PrismSynthetic\workspace-{directory:05}\projects\documents\source-and-notes\{number:08}-{label}.md"
        ),
        is_dir: number.is_multiple_of(97),
        size: number as u64 * 17,
        modified_ms: Some(1609459200000 + number as i64),
    }
}
fn checksum(entries: &BTreeMap<Arc<str>, Entry>) -> u64 {
    let mut state = DefaultHasher::new();
    for (key, entry) in entries {
        key.hash(&mut state);
        entry.path.hash(&mut state);
        entry.search.hash(&mut state);
        entry.initials_path.hash(&mut state);
        entry.keyboard_path.hash(&mut state);
        entry.name_start.hash(&mut state);
        entry.is_dir.hash(&mut state);
        entry.size.hash(&mut state);
        entry.modified_ms.hash(&mut state);
        entry.seen.hash(&mut state);
    }
    state.finish()
}
fn sharded(rows: Vec<Vec<FileRecord>>, workers: usize) -> BTreeMap<Arc<str>, Entry> {
    let (index, _owner) = isolated();
    index.prepare_initial_catalog(1, rows, 1, workers).unwrap()
}

#[test]
#[ignore = "synthetic client catalog timing, not C-drive or whole-app readiness"]
fn large_snapshot_catalog_assembly() {
    let count = std::env::var("PRISM_CATALOG_BENCH_RECORDS")
        .map_or(1_000_000, |value| value.parse::<usize>().unwrap());
    assert!((1..=2_500_000).contains(&count));
    let variants = std::env::var("PRISM_CATALOG_BENCH_VARIANT").map_or_else(
        |_| vec![0, 1, 4, 8],
        |value| vec![value.parse::<usize>().unwrap()],
    );
    assert!(variants.iter().all(|value| [0, 1, 4, 8].contains(value)));
    let repeats = std::env::var("PRISM_CATALOG_BENCH_REPEATS")
        .map_or(3, |value| value.parse::<usize>().unwrap());
    assert!((1..=3).contains(&repeats));
    let mut expected = None;
    for workers in variants {
        let mut samples = Vec::new();
        for _ in 0..repeats {
            let rows = rows(count);
            let (elapsed, entries) = if workers == 0 {
                let (index, _owner) = isolated();
                let start = Instant::now();
                let threads = std::thread::available_parallelism()
                    .map_or(2, usize::from)
                    .clamp(1, 8);
                index
                    .publish_service_rows(1, rows, 1, true, threads)
                    .unwrap();
                let elapsed = start.elapsed().as_millis();
                let entries = std::mem::take(&mut index.catalog.write().unwrap().entries);
                (elapsed, entries)
            } else {
                let start = Instant::now();
                let entries = sharded(rows, workers);
                (start.elapsed().as_millis(), entries)
            };
            assert_eq!(entries.len(), count);
            let hash = checksum(&entries);
            if let Some(expected) = expected {
                assert_eq!(hash, expected);
            } else {
                expected = Some(hash);
            }
            samples.push(elapsed);
        }
        samples.sort_unstable();
        println!("{{\"synthetic\":true,\"inputRecords\":{count},\"privateShards\":{workers},\"medianPrepareMs\":{},\"samples\":{:?},\"metadataChecksum\":\"{:016x}\"}}", samples[samples.len()/2], samples, expected.unwrap());
    }
}

#[test]
#[ignore = "synthetic complete producer/preparation timing and separate-process memory comparison"]
fn streaming_catalog_total_producer_and_preparation() {
    let count = std::env::var("PRISM_CATALOG_BENCH_RECORDS")
        .map_or(1_000_000, |value| value.parse::<usize>().unwrap());
    assert!((1..=2_500_000).contains(&count));
    let variants = std::env::var("PRISM_CATALOG_STREAM_VARIANT").map_or_else(
        |_| vec![false, true],
        |value| vec![value.parse::<bool>().unwrap()],
    );
    let mut expected = None;
    for streaming in variants {
        let (index, _owner) = isolated();
        // BOTH measurements include the exact same producer work. Streaming
        // does not get a free pre-generated full-snapshot allocation, nor is a
        // fake network delay inserted to manufacture overlap benefits.
        let start = Instant::now();
        let prepared = if streaming {
            std::thread::scope(|scope| {
                let preparation = pipeline::Preparation::start(scope, &index, 1, 1, 8).unwrap();
                for first in (0..count).step_by(2048) {
                    preparation
                        .send((first..(first + 2048).min(count)).map(row).collect())
                        .unwrap();
                }
                preparation.finish().unwrap()
            })
        } else {
            index.prepare_initial_catalog(1, rows(count), 1, 8).unwrap()
        };
        let elapsed = start.elapsed().as_millis();
        assert_eq!(prepared.len(), count);
        let hash = checksum(&prepared);
        if let Some(expected) = expected {
            assert_eq!(hash, expected);
        } else {
            expected = Some(hash);
        }
        println!("{{\"synthetic\":true,\"streaming\":{streaming},\"inputRecords\":{count},\"producerAndPrepareMs\":{elapsed},\"metadataChecksum\":\"{hash:016x}\"}}");
    }
}

#[cfg(windows)]
#[test]
#[ignore = "explicit ordinary-user complete C-service streaming preparation; aggregate data only"]
fn live_streaming_installed_service_catalog() {
    use prism_ntfs_windows::{administration, client, scm};
    assert_eq!(
        std::env::var("PRISM_TEST_FILENAME_LIVE_SERVICE").as_deref(),
        Ok("C")
    );
    assert!(!administration::is_elevated().unwrap());
    assert_eq!(scm::state().unwrap(), scm::State::Running);
    let (index, _owner) = isolated();
    let mut received = 0usize;
    let mut wire = (0u64, 0u64);
    let start = Instant::now();
    let mut transfer_done = None;
    index
        .collect_service_snapshot(1, Path::new(r"C:\"), 1, |cancel, receive| {
            let result = client::snapshot(Path::new(r"C:\"), cancel, |event| {
                if let Event::Records(rows) = &event {
                    received += rows.len();
                    for row in rows {
                        add_metadata(&mut wire, &row.path, row.is_dir, row.size, row.modified_ms);
                    }
                }
                receive(event)
            });
            transfer_done = Some(start.elapsed());
            result
        })
        .unwrap();
    let total = start.elapsed();
    let catalog = index.catalog.read().unwrap();
    let mut stored = (0u64, 0u64);
    for row in catalog.entries.values() {
        add_metadata(
            &mut stored,
            &row.path,
            row.is_dir,
            row.size,
            row.modified_ms,
        );
    }
    assert_eq!(received, catalog.entries.len());
    assert_eq!(
        stored, wire,
        "path and complete metadata aggregate must survive"
    );
    assert!(received > 0);
    let transferred = transfer_done.unwrap();
    // Transfer includes private work/backpressure, not pure wire I/O. Only the
    // tail after successful native/root validation is reported separately.
    println!("{{\"synthetic\":false,\"ordinaryClient\":true,\"streaming\":true,\"received\":{received},\"indexed\":{},\"validatedTransferWithPreparationMs\":{},\"joinMergePublishTailMs\":{},\"totalMs\":{},\"metadataMatches\":true}}", catalog.entries.len(), transferred.as_millis(), (total-transferred).as_millis(), total.as_millis());
}

#[cfg(windows)]
fn add_metadata(
    total: &mut (u64, u64),
    path: &str,
    is_dir: bool,
    size: u64,
    modified: Option<i64>,
) {
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    is_dir.hash(&mut hasher);
    size.hash(&mut hasher);
    modified.hash(&mut hasher);
    let value = hasher.finish();
    total.0 ^= value;
    total.1 = total.1.wrapping_add(value);
}

#[cfg(windows)]
#[test]
#[ignore = "explicit ordinary-user installed C-service read; aggregate timings only"]
fn live_installed_service_catalog_preparation() {
    use prism_ntfs_protocol::DriveLetter;
    use prism_ntfs_windows::{administration, client, scm};
    assert_eq!(
        std::env::var("PRISM_TEST_FILENAME_LIVE_SERVICE").as_deref(),
        Ok("C")
    );
    assert!(
        !administration::is_elevated().unwrap(),
        "use ordinary privileges"
    );
    assert_eq!(scm::state().unwrap(), scm::State::Running);
    let drive = DriveLetter::new(b'C').unwrap();
    for mode in [0, 8] {
        let (index, _owner) = isolated();
        let mut rows = Vec::new();
        let mut received = 0usize;
        let total = Instant::now();
        client::snapshot(
            Path::new(r"C:\"),
            Arc::new(AtomicBool::new(false)),
            |event| {
                match event {
                    Event::Records(batch) => {
                        for row in &batch {
                            assert!(drive.contains(&row.path));
                        }
                        received += batch.len();
                        rows.push(batch);
                    }
                    Event::Progress { .. } => {}
                    _ => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "Unexpected snapshot event.",
                        ))
                    }
                }
                Ok(())
            },
        )
        .unwrap();
        let fetch_ms = total.elapsed().as_millis();
        let preparation = Instant::now();
        let workers = std::thread::available_parallelism()
            .map_or(2, usize::from)
            .clamp(1, 8);
        if mode == 0 {
            index
                .publish_service_rows(1, rows, 1, true, workers)
                .unwrap();
        } else {
            let prepared = index.prepare_initial_catalog(1, rows, 1, workers).unwrap();
            index.publish_initial_catalog(1, prepared).unwrap();
        }
        let prepare_ms = preparation.elapsed().as_millis();
        let indexed = index.status().unwrap().count;
        assert!(received > 0 && indexed > 0);
        println!("{{\"synthetic\":false,\"ordinaryClient\":true,\"privateCatalog\":{},\"received\":{received},\"indexed\":{indexed},\"validatedFetchMs\":{fetch_ms},\"preparePublishMs\":{prepare_ms},\"totalMs\":{}}}", mode > 0, total.elapsed().as_millis());
    }
}

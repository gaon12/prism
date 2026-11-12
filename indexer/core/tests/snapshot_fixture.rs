//! Real public MFT/volume bytes with a deterministic driver/journal fixture for
//! changes occurring during a scan. The simulated journal is not evidence of
//! live Windows journal execution; it verifies snapshot publication ordering.
use flate2::read::GzDecoder;
use prism_ntfs_core::{
    journal::{self, Checkpoint},
    layout::{self, Error, Geometry, RecordBuffer},
    snapshot, FileReference,
};
use prism_ntfs_protocol::DriveLetter;
use std::{cell::Cell, collections::BTreeMap, io::Read};

struct ChangingImage {
    volume: Vec<u8>,
    mft: Vec<u8>,
    driver: BTreeMap<u64, Option<Vec<u8>>>,
    events: Vec<FileReference>,
    points: usize,
    journal_reads: usize,
    wrap: bool,
    next: i64,
    journal_from: i64,
    forbid_raw: bool,
    record_reads: usize,
}
fn inflate(bytes: &[u8]) -> Vec<u8> {
    let mut output = vec![];
    GzDecoder::new(bytes)
        .take(3 * 1024 * 1024)
        .read_to_end(&mut output)
        .unwrap();
    output
}
fn geometry() -> Geometry {
    Geometry {
        sector_size: 512,
        cluster_size: 512,
        record_size: 1024,
        volume_bytes: 2097152,
        mft_bytes: 594944,
        serial: 1,
    }
}

#[cfg(feature = "cache")]
#[test]
fn verified_cache_stream_matches_the_public_image_after_journal_replay() {
    use prism_ntfs_core::snapshot::codec::{self, streaming::Reader};
    use std::io::Cursor;
    let mut source = ChangingImage::new();
    let mut snapshot = snapshot::collect(&mut source, geometry(), || Ok(()), |_| {}).unwrap();
    let device = r"\Device\PublicCacheFixture";
    let mut saved = Cursor::new(Vec::new());
    codec::write(&snapshot, device, &mut saved, || Ok(())).unwrap();
    let bytes = saved.into_inner();
    let mut streaming =
        Reader::open(Cursor::new(bytes.clone()), geometry(), device, || Ok(())).unwrap();
    assert!(streaming.directory_count() < streaming.record_count());
    for letter in *b"CDC" {
        let drive = DriveLetter::new(letter).unwrap();
        let mut expected = Vec::new();
        snapshot
            .catalog
            .visit(drive, |row| {
                expected.push(row);
                Ok::<_, ()>(())
            })
            .unwrap();
        let mut actual = Vec::new();
        streaming
            .visit(
                drive,
                || Ok(()),
                |row| {
                    actual.push(row);
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(actual, expected);
    }
    assert_eq!(streaming.checkpoint(), snapshot.checkpoint);
    assert!(Reader::open(
        Cursor::new(bytes),
        geometry(),
        r"\Device\OtherVolume",
        || Ok(())
    )
    .is_err());
}
fn reference(number: u64, record: &[u8]) -> FileReference {
    FileReference(
        number | (u64::from(u16::from_le_bytes(record[16..18].try_into().unwrap())) << 48),
    )
}
fn rename(bytes: &mut [u8], old: &str, new: &str) {
    let encoded = |text: &str| {
        text.encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>()
    };
    let old = encoded(old);
    let new = encoded(new);
    assert_eq!(old.len(), new.len());
    let offset = bytes
        .windows(old.len())
        .position(|part| part == old)
        .unwrap();
    bytes[offset..offset + old.len()].copy_from_slice(&new);
}
impl ChangingImage {
    fn new() -> Self {
        Self {
            volume: inflate(include_bytes!("fixtures/testfs1-volume.gz")),
            mft: inflate(include_bytes!("fixtures/testfs1-mft.gz")),
            driver: BTreeMap::new(),
            events: vec![],
            points: 0,
            journal_reads: 0,
            wrap: false,
            next: 1024,
            journal_from: 128,
            forbid_raw: false,
            record_reads: 0,
        }
    }
    fn record(&self, number: u64) -> Vec<u8> {
        self.mft[number as usize * 1024..(number as usize + 1) * 1024].to_owned()
    }
    fn changes(&mut self) {
        let deleted = reference(64, &self.record(64));
        self.driver.insert(64, None);
        self.events.push(deleted);
        let mut renamed = self.record(65);
        rename(&mut renamed, "file-with-12345", "file-with-67890");
        self.events.push(reference(65, &renamed));
        self.driver.insert(65, Some(renamed));
        let mut directory = self.record(68);
        rename(&mut directory, "many_subdirs", "more_subdirs");
        self.events.push(reference(68, &directory));
        self.driver.insert(68, Some(directory));
        let mut reused = self.record(66);
        self.events.push(reference(66, &reused));
        let sequence = u16::from_le_bytes(reused[16..18].try_into().unwrap()) + 1;
        reused[16..18].copy_from_slice(&sequence.to_le_bytes());
        self.events.push(reference(66, &reused));
        self.driver.insert(66, Some(reused));
    }
}
impl layout::Source for ChangingImage {
    fn read_exact_at(&mut self, offset: u64, output: &mut [u8]) -> Result<(), Error> {
        assert!(
            !self.forbid_raw,
            "incremental fixture must not read the MFT again"
        );
        let start = offset as usize;
        output.copy_from_slice(
            self.volume
                .get(start..start + output.len())
                .ok_or(Error::Corrupt)?,
        );
        Ok(())
    }
    fn file_record(&mut self, wanted: FileReference) -> Result<RecordBuffer, Error> {
        self.record_reads += 1;
        let bytes = match self.driver.get(&wanted.number()) {
            Some(record) => record.clone().ok_or(Error::Changed)?,
            None => self.record(wanted.number()),
        };
        if wanted.sequence() != 0 && reference(wanted.number(), &bytes) != wanted {
            return Err(Error::Changed);
        }
        Ok(RecordBuffer {
            bytes,
            kernel_fixups: false,
        })
    }
}
impl journal::Source for ChangingImage {
    fn checkpoint(&mut self) -> Result<Checkpoint, Error> {
        self.points += 1;
        Ok(Checkpoint {
            id: if self.wrap && self.points > 2 { 8 } else { 7 },
            first: 0,
            next: if self.points == 1 { 128 } else { self.next },
            lowest: 0,
        })
    }
    fn read_changes(&mut self, id: u64, from: i64) -> Result<Vec<u8>, Error> {
        assert_eq!(id, 7);
        assert_eq!(from, self.journal_from);
        self.journal_reads += 1;
        let mut bytes = self.next.to_le_bytes().to_vec();
        for (index, reference) in self.events.iter().enumerate() {
            let mut record = vec![0; 64];
            record[..4].copy_from_slice(&64u32.to_le_bytes());
            record[4..6].copy_from_slice(&2u16.to_le_bytes());
            record[8..16].copy_from_slice(&reference.0.to_le_bytes());
            record[24..32].copy_from_slice(&(self.journal_from + index as i64 * 64).to_le_bytes());
            record[56..58].copy_from_slice(&2u16.to_le_bytes());
            record[58..60].copy_from_slice(&60u16.to_le_bytes());
            record[60] = b'x';
            bytes.extend(record);
        }
        Ok(bytes)
    }
}

#[test]
fn only_publishes_after_replaying_deletion_rename_directory_move_and_id_reuse() {
    let mut source = ChangingImage::new();
    source.changes();
    let mut progress = vec![];
    let mut snapshot = snapshot::collect(
        &mut source,
        geometry(),
        || Ok(()),
        |next| progress.push(next),
    )
    .unwrap();
    assert_eq!(snapshot.records_read, 581);
    assert_eq!(snapshot.checkpoint.next, 1024);
    assert_eq!(source.journal_reads, 1);
    let mut files = vec![];
    snapshot
        .catalog
        .visit(DriveLetter::new(b'C').unwrap(), |file| {
            files.push(file);
            Ok::<_, ()>(())
        })
        .unwrap();
    assert!(!files.iter().any(|file| file.path == "C:\\empty-file"
        || file.path == "C:\\file-with-12345"
        || file.path.starts_with("C:\\many_subdirs")));
    assert_eq!(
        files
            .iter()
            .find(|file| file.path == "C:\\file-with-67890")
            .unwrap()
            .size,
        5
    );
    assert_eq!(
        files
            .iter()
            .find(|file| file.path == "C:\\1000-bytes-file")
            .unwrap()
            .size,
        1000
    );
    assert_eq!(
        files
            .iter()
            .filter(|file| file.path.starts_with("C:\\more_subdirs\\"))
            .count(),
        512
    );
    assert!(progress.iter().any(|phase| matches!(
        phase,
        snapshot::Progress::Mft {
            processed: 581,
            total: 581
        }
    )));
    assert!(progress.iter().any(|phase| matches!(
        phase,
        snapshot::Progress::Replay {
            processed: 5,
            total: 5
        }
    )));
}
#[test]
fn a_torn_live_record_is_repaired_through_the_driver_instead_of_lost() {
    let mut source = ChangingImage::new();
    let record = source.record(65);
    let offset = source
        .volume
        .windows(1024)
        .position(|part| part == record)
        .unwrap();
    source.volume[offset + 510] ^= 1;
    let mut snapshot = snapshot::collect(&mut source, geometry(), || Ok(()), |_| {}).unwrap();
    let mut found = false;
    snapshot
        .catalog
        .visit(DriveLetter::new(b'C').unwrap(), |file| {
            if file.path == "C:\\file-with-12345" {
                found = true;
                assert_eq!(file.size, 5);
            }
            Ok::<_, ()>(())
        })
        .unwrap();
    assert!(found);
}
#[test]
fn journal_replacement_and_cancellation_return_no_partial_snapshot() {
    let mut source = ChangingImage::new();
    source.wrap = true;
    assert!(matches!(
        snapshot::collect(&mut source, geometry(), || Ok(()), |_| {}),
        Err(Error::JournalGap)
    ));
    let mut source = ChangingImage::new();
    let cancel = Cell::new(false);
    assert!(matches!(
        snapshot::collect(
            &mut source,
            geometry(),
            || if cancel.get() {
                Err(Error::Cancelled)
            } else {
                Ok(())
            },
            |phase| if matches!(phase,snapshot::Progress::Mft{processed,..} if processed>=256) {
                cancel.set(true)
            }
        ),
        Err(Error::Cancelled)
    ));
    assert_eq!(source.journal_reads, 0);
}

fn paths(snapshot: &mut snapshot::Snapshot) -> Vec<String> {
    let mut paths = vec![];
    snapshot
        .catalog
        .visit(DriveLetter::new(b'C').unwrap(), |record| {
            paths.push(record.path);
            Ok::<_, ()>(())
        })
        .unwrap();
    paths
}

#[test]
fn incremental_replay_updates_renames_deletions_descendants_and_reused_ids_without_mft_reads() {
    let mut source = ChangingImage::new();
    let mut first = snapshot::collect(&mut source, geometry(), || Ok(()), |_| {}).unwrap();
    assert!(paths(&mut first)
        .iter()
        .any(|path| path.starts_with("C:\\many_subdirs\\")));
    source.changes();
    source.forbid_raw = true;
    source.journal_from = first.checkpoint.next;
    source.next = 2048;
    source.record_reads = 0;
    let mut next = snapshot::refresh(first, &mut source, geometry(), || Ok(()), |_| {}).unwrap();
    assert_eq!(next.records_read, 0);
    assert_eq!(next.checkpoint.next, 2048);
    assert!(
        source.record_reads <= 5,
        "only changed references are re-read"
    );
    let files = paths(&mut next);
    assert!(!files.iter().any(|path| path == "C:\\empty-file"
        || path == "C:\\file-with-12345"
        || path.starts_with("C:\\many_subdirs")));
    assert!(files.iter().any(|path| path == "C:\\file-with-67890"));
    assert!(files.iter().any(|path| path == "C:\\1000-bytes-file"));
    assert_eq!(
        files
            .iter()
            .filter(|path| path.starts_with("C:\\more_subdirs\\"))
            .count(),
        512
    );
    // A second, unchanged interval must not read records or journal batches.
    source.journal_reads = 0;
    source.record_reads = 0;
    let mut unchanged =
        snapshot::refresh(next, &mut source, geometry(), || Ok(()), |_| {}).unwrap();
    assert_eq!(paths(&mut unchanged), files);
    assert_eq!((source.journal_reads, source.record_reads), (0, 0));
}

fn extended_snapshot(source: &ChangingImage) -> snapshot::Snapshot {
    use prism_ntfs_core::{parse_record, CatalogBuilder};
    let mut builder = CatalogBuilder::default();
    let root = parse_record(5, 512, &source.record(5)).unwrap().unwrap();
    let base = parse_record(65, 512, &source.record(65)).unwrap().unwrap();
    let mut extra = base.clone();
    extra.reference = FileReference((9u64 << 48) | 42);
    extra.base = Some(base.reference);
    extra.names[0].name = "extra.md".into();
    builder.insert(root).unwrap();
    builder.insert(base).unwrap();
    builder.insert(extra).unwrap();
    snapshot::Snapshot {
        catalog: builder.finish().unwrap(),
        checkpoint: Checkpoint {
            id: 7,
            first: 0,
            next: 128,
            lowest: 0,
        },
        geometry: geometry(),
        records_read: 581,
    }
}

#[cfg(feature = "cache")]
fn cached_pair(
    snapshot: &snapshot::Snapshot,
) -> (
    snapshot::Snapshot,
    snapshot::codec::streaming::Reader<std::io::Cursor<Vec<u8>>>,
) {
    use snapshot::codec::{self, streaming::Reader};
    let mut saved = std::io::Cursor::new(Vec::new());
    codec::write(snapshot, r"\Device\ReplayFixture", &mut saved, || Ok(())).unwrap();
    let bytes = saved.into_inner();
    let full = codec::read(
        &mut std::io::Cursor::new(bytes.clone()),
        geometry(),
        r"\Device\ReplayFixture",
        || Ok(()),
    )
    .unwrap();
    let stream = Reader::open(
        std::io::Cursor::new(bytes),
        geometry(),
        r"\Device\ReplayFixture",
        || Ok(()),
    )
    .unwrap();
    (full, stream)
}

#[cfg(feature = "cache")]
fn compare_replayed(
    full: &mut snapshot::Snapshot,
    stream: &mut snapshot::codec::streaming::Reader<std::io::Cursor<Vec<u8>>>,
) -> Vec<prism_ntfs_protocol::FileRecord> {
    let mut last = Vec::new();
    for letter in *b"CDC" {
        let drive = DriveLetter::new(letter).unwrap();
        let mut expected = Vec::new();
        full.catalog
            .visit(drive, |row| {
                expected.push(row);
                Ok::<_, ()>(())
            })
            .unwrap();
        let mut actual = Vec::new();
        stream
            .visit(
                drive,
                || Ok(()),
                |row| {
                    actual.push(row);
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(actual, expected);
        last = actual;
    }
    assert_eq!(stream.checkpoint(), full.checkpoint);
    let mut references = Vec::new();
    stream
        .visit_metadata(
            || Ok(()),
            |record| {
                references.push(record.reference);
                Ok(())
            },
        )
        .unwrap();
    for reference in &references {
        assert_eq!(stream.reference(reference.number()), Some(*reference));
    }
    assert_eq!(references.len(), stream.record_count());
    assert!(references
        .windows(2)
        .all(|pair| pair[0].number() < pair[1].number()));
    last
}

#[cfg(feature = "cache")]
#[test]
fn sequential_cache_replay_matches_full_across_renames_deletions_reuse_and_two_intervals() {
    let mut initial = ChangingImage::new();
    let first = snapshot::collect(&mut initial, geometry(), || Ok(()), |_| {}).unwrap();
    let (full, stream) = cached_pair(&first);
    let prepare = || {
        let mut source = ChangingImage::new();
        source.points = 1;
        source.forbid_raw = true;
        source.journal_from = first.checkpoint.next;
        source.next = 2048;
        source.changes();
        for (number, name) in [(40, "new-file-000040"), (600, "new-file-000600")] {
            let mut bytes = source.record(65);
            rename(&mut bytes, "file-with-12345", name);
            bytes[44..48].copy_from_slice(&(number as u32).to_le_bytes());
            source.events.push(reference(number, &bytes));
            source.driver.insert(number, Some(bytes));
        }
        source
    };
    let mut full_source = prepare();
    let mut stream_source = prepare();
    let mut full =
        snapshot::refresh(full, &mut full_source, geometry(), || Ok(()), |_| {}).unwrap();
    let mut progress = Vec::new();
    let mut stream = stream
        .refresh(
            &mut stream_source,
            geometry(),
            || Ok(()),
            |next| progress.push(next),
        )
        .unwrap();
    let rows = compare_replayed(&mut full, &mut stream);
    assert!(rows.iter().any(|row| row.path == "C:\\new-file-000040"));
    assert!(rows.iter().any(|row| row.path == "C:\\new-file-000600"));
    assert_eq!(
        rows.iter()
            .filter(|row| row.path.starts_with("C:\\more_subdirs\\"))
            .count(),
        512
    );
    assert!(!rows
        .iter()
        .any(|row| row.path == "C:\\empty-file" || row.path == "C:\\file-with-12345"));
    assert_eq!(stream_source.record_reads, full_source.record_reads);
    assert!(stream_source.record_reads <= 7);
    assert!(matches!(
        progress.last(),
        Some(snapshot::Progress::Replay {
            processed: 7,
            total: 7
        })
    ));
    for source in [&mut full_source, &mut stream_source] {
        source.events.clear();
        source.journal_from = 2048;
        source.next = 3072;
        let removed = reference(40, source.driver[&40].as_ref().unwrap());
        source.events.push(removed);
        source.driver.insert(40, None);
        for number in [65, 68] {
            let bytes = source.record(number);
            source.events.push(reference(number, &bytes));
            source.driver.insert(number, Some(bytes));
        }
        source.record_reads = 0;
    }
    full = snapshot::refresh(full, &mut full_source, geometry(), || Ok(()), |_| {}).unwrap();
    stream = stream
        .refresh(&mut stream_source, geometry(), || Ok(()), |_| {})
        .unwrap();
    let rows = compare_replayed(&mut full, &mut stream);
    assert!(
        !rows
            .iter()
            .any(|row| row.path == "C:\\new-file-000040"
                || row.path.starts_with("C:\\more_subdirs\\"))
    );
    assert_eq!(
        rows.iter()
            .filter(|row| row.path.starts_with("C:\\many_subdirs\\"))
            .count(),
        512
    );
    assert!(rows.iter().any(|row| row.path == "C:\\file-with-12345"));
    assert_eq!(stream_source.record_reads, 3);
    // An unchanged third interval keeps the overlay and performs no record or journal reads.
    stream_source.record_reads = 0;
    stream_source.journal_reads = 0;
    stream = stream
        .refresh(&mut stream_source, geometry(), || Ok(()), |_| {})
        .unwrap();
    assert_eq!(compare_replayed(&mut full, &mut stream), rows);
    assert_eq!(
        (stream_source.record_reads, stream_source.journal_reads),
        (0, 0)
    );
    // Persist the replayed checkpoint in the original cache format, reopen it
    // and replay another unchanged interval without raw MFT reads.
    let mut expected = std::io::Cursor::new(Vec::new());
    snapshot::codec::write(&full, r"\Device\ReplayFixture", &mut expected, || Ok(())).unwrap();
    let mut saved = std::io::Cursor::new(Vec::new());
    stream
        .write(r"\Device\ReplayFixture", &mut saved, || Ok(()))
        .unwrap();
    assert_eq!(saved.get_ref(), expected.get_ref());
    let reopened = snapshot::codec::streaming::Reader::open(
        saved,
        geometry(),
        r"\Device\ReplayFixture",
        || Ok(()),
    )
    .unwrap();
    let mut reopened = reopened
        .refresh(&mut stream_source, geometry(), || Ok(()), |_| {})
        .unwrap();
    assert_eq!(compare_replayed(&mut full, &mut reopened), rows);
    assert_eq!(
        (stream_source.record_reads, stream_source.journal_reads),
        (0, 0)
    );
}

#[cfg(feature = "cache")]
#[test]
fn sequential_replay_keeps_old_extension_ownership_when_the_extension_is_deleted_or_reused() {
    for reuse in [false, true] {
        let first = extended_snapshot(&ChangingImage::new());
        let (full, stream) = cached_pair(&first);
        let prepare = || {
            let mut source = ChangingImage::new();
            source.points = 1;
            source.forbid_raw = true;
            if reuse {
                let mut bytes = source.record(65);
                rename(&mut bytes, "file-with-12345", "other-file-0000");
                bytes[16..18].copy_from_slice(&10u16.to_le_bytes());
                bytes[44..48].copy_from_slice(&42u32.to_le_bytes());
                source.events.push(reference(42, &bytes));
                source.driver.insert(42, Some(bytes));
            } else {
                source.events.push(FileReference((9 << 48) | 42));
                source.driver.insert(42, None);
            }
            source
        };
        let mut full_source = prepare();
        let mut stream_source = prepare();
        let mut full =
            snapshot::refresh(full, &mut full_source, geometry(), || Ok(()), |_| {}).unwrap();
        let mut stream = stream
            .refresh(&mut stream_source, geometry(), || Ok(()), |_| {})
            .unwrap();
        let rows = compare_replayed(&mut full, &mut stream);
        assert!(rows.iter().any(|row| row.path == "C:\\file-with-12345"));
        assert!(!rows.iter().any(|row| row.path == "C:\\extra.md"));
        assert_eq!(
            rows.iter().any(|row| row.path == "C:\\other-file-0000"),
            reuse
        );
        assert_eq!(stream.owner(FileReference((9 << 48) | 42)), None);
        assert_eq!(stream_source.record_reads, full_source.record_reads);
    }
}

#[cfg(feature = "cache")]
#[test]
fn sequential_replay_returns_no_reader_on_cancel_gap_or_wrong_geometry() {
    let mut initial = ChangingImage::new();
    let first = snapshot::collect(&mut initial, geometry(), || Ok(()), |_| {}).unwrap();
    for error in [Error::Cancelled, Error::TimedOut] {
        let (_, stream) = cached_pair(&first);
        let mut source = ChangingImage::new();
        source.points = 1;
        source.forbid_raw = true;
        source.journal_from = first.checkpoint.next;
        source.next = 2048;
        source.changes();
        let stop = Cell::new(false);
        assert!(
            matches!(stream.refresh(&mut source, geometry(), || if stop.get() { Err(error) } else { Ok(()) }, |_| stop.set(true)), Err(actual) if actual == error)
        );
        assert_eq!(
            source.record_reads, 1,
            "one private change precedes cancellation"
        );
    }
    let (_, stream) = cached_pair(&first);
    let mut source = ChangingImage::new();
    source.points = 1;
    source.forbid_raw = true;
    source.wrap = true;
    assert!(matches!(
        stream.refresh(&mut source, geometry(), || Ok(()), |_| {}),
        Err(Error::JournalGap)
    ));
    let (_, stream) = cached_pair(&first);
    let mut source = ChangingImage::new();
    let mut wrong = geometry();
    wrong.serial += 1;
    assert!(matches!(
        stream.refresh(&mut source, wrong, || Ok(()), |_| {}),
        Err(Error::Changed)
    ));
    assert_eq!(
        (source.points, source.record_reads, source.journal_reads),
        (0, 0, 0)
    );
}

#[test]
fn a_deleted_or_reused_extension_refreshes_its_old_surviving_base() {
    for reuse in [false, true] {
        let mut source = ChangingImage::new();
        let mut old = extended_snapshot(&source);
        #[cfg(feature = "cache")]
        {
            let mut bytes = std::io::Cursor::new(Vec::new());
            snapshot::codec::write(&old, r"\Device\Fixture", &mut bytes, || Ok(())).unwrap();
            old = snapshot::codec::read(&mut bytes, geometry(), r"\Device\Fixture", || Ok(()))
                .unwrap();
        }
        assert!(paths(&mut old).iter().any(|path| path == "C:\\extra.md"));
        source.points = 1;
        source.forbid_raw = true;
        if reuse {
            let mut new = source.record(65);
            rename(&mut new, "file-with-12345", "other-file-0000");
            new[16..18].copy_from_slice(&10u16.to_le_bytes());
            new[44..48].copy_from_slice(&42u32.to_le_bytes());
            source.events.push(reference(42, &new));
            source.driver.insert(42, Some(new));
        } else {
            source.events.push(FileReference((9u64 << 48) | 42));
            source.driver.insert(42, None);
        }
        // The current real fixture base no longer has an attribute list, so
        // replaying the OLD extension relationship must remove extra.md while
        // preserving the base's own filename. The journal fixture emits no
        // base event: dropping that relationship would retain the stale name.
        let mut next = snapshot::refresh(old, &mut source, geometry(), || Ok(()), |_| {}).unwrap();
        let files = paths(&mut next);
        assert!(files.iter().any(|path| path == "C:\\file-with-12345"));
        assert!(!files.iter().any(|path| path == "C:\\extra.md"));
        assert_eq!(
            files.iter().any(|path| path == "C:\\other-file-0000"),
            reuse
        );
    }
}

#[test]
fn incremental_cancellation_journal_loss_and_other_volume_never_return_partial_catalogs() {
    let mut source = ChangingImage::new();
    let first = snapshot::collect(&mut source, geometry(), || Ok(()), |_| {}).unwrap();
    source.changes();
    source.journal_from = first.checkpoint.next;
    source.next = 2048;
    source.forbid_raw = true;
    let cancel = Cell::new(false);
    assert!(matches!(
        snapshot::refresh(
            first,
            &mut source,
            geometry(),
            || {
                if cancel.get() {
                    Err(Error::Cancelled)
                } else {
                    Ok(())
                }
            },
            |_| cancel.set(true)
        ),
        Err(Error::Cancelled)
    ));
    let mut source = ChangingImage::new();
    let first = snapshot::collect(&mut source, geometry(), || Ok(()), |_| {}).unwrap();
    source.wrap = true;
    source.forbid_raw = true;
    assert!(matches!(
        snapshot::refresh(first, &mut source, geometry(), || Ok(()), |_| {}),
        Err(Error::JournalGap)
    ));
    let mut source = ChangingImage::new();
    let first = snapshot::collect(&mut source, geometry(), || Ok(()), |_| {}).unwrap();
    let points = source.points;
    let mut other = geometry();
    other.serial += 1;
    assert!(matches!(
        snapshot::refresh(first, &mut source, other, || Ok(()), |_| {}),
        Err(Error::Changed)
    ));
    assert_eq!(source.points, points, "reject the other volume before I/O");
}

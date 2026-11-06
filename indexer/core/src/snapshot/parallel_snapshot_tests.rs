//! Force the actual snapshot coordinator through multiple parser jobs on the
//! independent public filesystem image. No privileged disk or live journal.
use super::*;
use crate::layout::{RecordBuffer, Source};
use flate2::read::GzDecoder;
use prism_ntfs_protocol::{DriveLetter, FileRecord};
use std::{
    cell::Cell,
    collections::BTreeMap,
    io::Read,
    thread::{self, ThreadId},
};

struct Image {
    volume: Vec<u8>,
    mft: Vec<u8>,
    driver: BTreeMap<u64, Option<Vec<u8>>>,
    events: Vec<FileReference>,
    points: usize,
    journal_reads: usize,
    wrap: bool,
    owner: ThreadId,
}
fn inflate(bytes: &[u8]) -> Vec<u8> {
    let mut output = Vec::new();
    GzDecoder::new(bytes).read_to_end(&mut output).unwrap();
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
fn reference(number: u64, bytes: &[u8]) -> FileReference {
    FileReference(number | (u64::from(u16::from_le_bytes(bytes[16..18].try_into().unwrap())) << 48))
}
impl Image {
    fn new() -> Self {
        Self {
            volume: inflate(include_bytes!("../../tests/fixtures/testfs1-volume.gz")),
            mft: inflate(include_bytes!("../../tests/fixtures/testfs1-mft.gz")),
            driver: BTreeMap::new(),
            events: Vec::new(),
            points: 0,
            journal_reads: 0,
            wrap: false,
            owner: thread::current().id(),
        }
    }
    fn record(&self, number: u64) -> Vec<u8> {
        self.mft[number as usize * 1024..(number as usize + 1) * 1024].to_vec()
    }
    fn change(&mut self, number: u64, old: &str, new: &str) {
        let mut record = self.record(number);
        let encoded = |text: &str| {
            text.encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>()
        };
        let old = encoded(old);
        let new = encoded(new);
        assert_eq!(old.len(), new.len());
        let offset = record
            .windows(old.len())
            .position(|part| part == old)
            .unwrap();
        record[offset..offset + new.len()].copy_from_slice(&new);
        self.events.push(reference(number, &record));
        self.driver.insert(number, Some(record));
    }
    fn changes(&mut self) {
        self.events.push(reference(64, &self.record(64)));
        self.driver.insert(64, None);
        self.change(65, "file-with-12345", "file-with-67890");
        self.change(68, "many_subdirs", "more_subdirs");
        let mut reused = self.record(66);
        self.events.push(reference(66, &reused));
        let sequence = u16::from_le_bytes(reused[16..18].try_into().unwrap()) + 1;
        reused[16..18].copy_from_slice(&sequence.to_le_bytes());
        self.events.push(reference(66, &reused));
        self.driver.insert(66, Some(reused));
    }
}
impl Source for Image {
    fn read_exact_at(&mut self, offset: u64, output: &mut [u8]) -> layout::Result<()> {
        assert_eq!(
            thread::current().id(),
            self.owner,
            "volume I/O stays on coordinator"
        );
        output.copy_from_slice(
            self.volume
                .get(offset as usize..offset as usize + output.len())
                .ok_or(Error::Corrupt)?,
        );
        Ok(())
    }
    fn file_record(&mut self, wanted: FileReference) -> layout::Result<RecordBuffer> {
        assert_eq!(
            thread::current().id(),
            self.owner,
            "driver I/O stays on coordinator"
        );
        let bytes = match self.driver.get(&wanted.number()) {
            Some(record) => record.clone().ok_or(Error::Changed)?,
            None => self.record(wanted.number()),
        };
        if wanted.sequence() != 0 && wanted != reference(wanted.number(), &bytes) {
            return Err(Error::Changed);
        }
        Ok(RecordBuffer {
            bytes,
            kernel_fixups: false,
        })
    }
}
impl journal::Source for Image {
    fn checkpoint(&mut self) -> layout::Result<journal::Checkpoint> {
        assert_eq!(
            thread::current().id(),
            self.owner,
            "journal I/O stays on coordinator"
        );
        self.points += 1;
        Ok(journal::Checkpoint {
            id: if self.wrap && self.points > 2 { 8 } else { 7 },
            first: 0,
            next: if self.points == 1 { 128 } else { 1024 },
            lowest: 0,
        })
    }
    fn read_changes(&mut self, id: u64, from: i64) -> layout::Result<Vec<u8>> {
        assert_eq!(thread::current().id(), self.owner);
        assert_eq!((id, from), (7, 128));
        self.journal_reads += 1;
        let mut output = 1024i64.to_le_bytes().to_vec();
        for (index, reference) in self.events.iter().enumerate() {
            let mut record = vec![0; 64];
            record[..4].copy_from_slice(&64u32.to_le_bytes());
            record[4..6].copy_from_slice(&2u16.to_le_bytes());
            record[8..16].copy_from_slice(&reference.0.to_le_bytes());
            record[24..32].copy_from_slice(&(128 + index as i64 * 64).to_le_bytes());
            record[56..58].copy_from_slice(&2u16.to_le_bytes());
            record[58..60].copy_from_slice(&60u16.to_le_bytes());
            record[60] = b'x';
            output.extend(record);
        }
        Ok(output)
    }
}
fn files(mut snapshot: Snapshot) -> Vec<FileRecord> {
    let mut result = Vec::new();
    assert_eq!(snapshot.records_read, 581);
    assert_eq!(snapshot.checkpoint.next, 1024);
    snapshot
        .catalog
        .visit(DriveLetter::new(b'C').unwrap(), |record| {
            result.push(record);
            Ok::<_, ()>(())
        })
        .unwrap();
    result
}

#[test]
fn parallel_snapshot_matches_sequential_after_torn_repair_and_full_journal_replay() {
    let mut reference = Image::new();
    reference.changes();
    let expected =
        files(collect_using(&mut reference, geometry(), || Ok(()), |_| {}, 1, 32 * 1024).unwrap());
    for workers in [2, 4, 8] {
        let mut source = Image::new();
        source.changes();
        let record = source.record(67);
        let offset = source
            .volume
            .windows(1024)
            .position(|part| part == record)
            .unwrap();
        source.volume[offset + 510] ^= 1;
        let owner = source.owner;
        let mut progress = Vec::new();
        let actual = collect_using(
            &mut source,
            geometry(),
            || {
                assert_eq!(
                    thread::current().id(),
                    owner,
                    "non-Sync checks stay on coordinator"
                );
                Ok(())
            },
            |phase| progress.push(phase),
            workers,
            32 * 1024,
        )
        .unwrap();
        assert_eq!(files(actual), expected);
        assert_eq!(source.journal_reads, 1);
        assert!(progress.contains(&Progress::Mft {
            processed: 581,
            total: 581
        }));
        assert!(progress.contains(&Progress::Replay {
            processed: 6,
            total: 6
        }));
    }
    assert_eq!(
        expected
            .iter()
            .filter(|file| file.path.starts_with("C:\\more_subdirs\\"))
            .count(),
        512
    );
    assert!(!expected
        .iter()
        .any(|file| file.path == "C:\\empty-file" || file.path.starts_with("C:\\many_subdirs\\")));
}

#[test]
fn parallel_cancel_deadline_and_journal_replacement_never_publish_partial_snapshots() {
    for failure in [Error::Cancelled, Error::TimedOut] {
        let mut source = Image::new();
        let stopped = Cell::new(false);
        let result = collect_using(
            &mut source,
            geometry(),
            || {
                if stopped.get() {
                    Err(failure)
                } else {
                    Ok(())
                }
            },
            |phase| {
                if matches!(phase, Progress::Mft {processed, ..} if processed >= 256) {
                    stopped.set(true);
                }
            },
            4,
            32 * 1024,
        );
        assert!(matches!(result, Err(error) if error == failure));
        assert_eq!(source.journal_reads, 0);
    }
    let mut source = Image::new();
    source.wrap = true;
    assert!(matches!(
        collect_using(&mut source, geometry(), || Ok(()), |_| {}, 4, 32 * 1024),
        Err(Error::JournalGap)
    ));
}

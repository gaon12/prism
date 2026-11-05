//! Bounds and validates USN replay. Journal records identify metadata to
//! re-read; their timestamps and filenames are never used as final file data.
use crate::layout::{Error, Result};
use crate::{slice, u16_at, u32_at, u64_at, FileReference};
use std::collections::BTreeSet;

const MAX_CHANGED_FILES: usize = 250_000;
const MAX_REPLAY_BYTES: usize = 128 * 1024 * 1024;
pub const MAX_JOURNAL_BATCH: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Checkpoint {
    pub id: u64,
    pub first: i64,
    pub next: i64,
    pub lowest: i64,
}
impl Checkpoint {
    pub fn validate(self) -> Result<Self> {
        if self.first < 0 || self.next < self.first || self.lowest < 0 || self.lowest > self.next {
            return Err(Error::Corrupt);
        }
        Ok(self)
    }
    fn permits(self, id: u64, from: i64) -> bool {
        self.id == id && from >= self.first.max(self.lowest) && from <= self.next
    }
}

pub trait Source {
    fn checkpoint(&mut self) -> Result<Checkpoint>;
    /// Return an immediate, non-waiting FSCTL_READ_USN_JOURNAL batch with
    /// reason mask all, close-only false, and supported record version 2.
    fn read_changes(&mut self, id: u64, from: i64) -> Result<Vec<u8>>;
}

pub struct Changes {
    pub references: Vec<FileReference>,
    pub checkpoint: Checkpoint,
}

/// Collect the closed range [start.next, stop.next). A driver can return a
/// later resume point because more changes arrive during the read; records at
/// or after stop are deliberately left for the next replay, not lost.
pub fn collect(
    source: &mut impl Source,
    start: Checkpoint,
    stop: Checkpoint,
    mut check: impl FnMut() -> Result<()>,
) -> Result<Changes> {
    let start = start.validate()?;
    let stop = stop.validate()?;
    if !stop.permits(start.id, start.next) {
        return Err(Error::JournalGap);
    }
    let mut cursor = start.next;
    let mut references = BTreeSet::new();
    let mut bytes_read = 0usize;
    while cursor < stop.next {
        check()?;
        let current = source.checkpoint()?.validate()?;
        if !current.permits(start.id, cursor) || current.next < stop.next {
            return Err(Error::JournalGap);
        }
        let bytes = source.read_changes(start.id, cursor)?;
        bytes_read = bytes_read.checked_add(bytes.len()).ok_or(Error::Limit)?;
        if bytes_read > MAX_REPLAY_BYTES {
            return Err(Error::Limit);
        }
        let next = decode_batch(&bytes, cursor, stop.next, &mut references)?;
        if next <= cursor {
            return Err(Error::Changed);
        }
        cursor = next;
    }
    check()?;
    // A replaced/wrapped journal invalidates the attempted snapshot even if
    // its last batch happened to be readable. A fresh snapshot can recover.
    let current = source.checkpoint()?.validate()?;
    if !current.permits(start.id, start.next) || current.next < stop.next {
        return Err(Error::JournalGap);
    }
    Ok(Changes {
        references: references.into_iter().map(FileReference).collect(),
        checkpoint: stop,
    })
}

fn decode_batch(bytes: &[u8], from: i64, to: i64, references: &mut BTreeSet<u64>) -> Result<i64> {
    if bytes.len() < 8 || bytes.len() > MAX_JOURNAL_BATCH {
        return Err(Error::Corrupt);
    }
    let next = u64_at(bytes, 0)? as i64;
    if next < from {
        return Err(Error::Corrupt);
    }
    let mut offset = 8;
    let mut last = from;
    while offset < bytes.len() {
        let length = u32_at(bytes, offset)? as usize;
        if length < 60 || !length.is_multiple_of(8) {
            return Err(Error::Corrupt);
        }
        let record = slice(bytes, offset, length)?;
        if u16_at(record, 4)? != 2 {
            return Err(Error::Unsupported);
        }
        let usn = u64_at(record, 24)? as i64;
        let name_length = usize::from(u16_at(record, 56)?);
        let name_offset = usize::from(u16_at(record, 58)?);
        if usn < last
            || usn >= next
            || name_offset < 60
            || !name_offset.is_multiple_of(2)
            || !name_length.is_multiple_of(2)
        {
            return Err(Error::Corrupt);
        }
        slice(record, name_offset, name_length)?;
        last = usn;
        if usn >= from && usn < to {
            references.insert(u64_at(record, 8)?);
            if references.len() > MAX_CHANGED_FILES {
                return Err(Error::Limit);
            }
        }
        offset = offset.checked_add(length).ok_or(Error::Corrupt)?;
    }
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    fn point(first: i64, next: i64) -> Checkpoint {
        Checkpoint {
            id: 7,
            first,
            next,
            lowest: 0,
        }
    }
    fn batch(next: i64, changes: &[(u64, i64)]) -> Vec<u8> {
        let mut bytes = next.to_le_bytes().to_vec();
        for (reference, usn) in changes {
            let mut record = vec![0; 64];
            record[..4].copy_from_slice(&64u32.to_le_bytes());
            record[4..6].copy_from_slice(&2u16.to_le_bytes());
            record[8..16].copy_from_slice(&reference.to_le_bytes());
            record[24..32].copy_from_slice(&usn.to_le_bytes());
            record[56..58].copy_from_slice(&2u16.to_le_bytes());
            record[58..60].copy_from_slice(&60u16.to_le_bytes());
            record[60] = b'x';
            bytes.extend(record);
        }
        bytes
    }
    struct Journal {
        points: VecDeque<Checkpoint>,
        batches: VecDeque<Vec<u8>>,
        calls: usize,
    }
    impl Source for Journal {
        fn checkpoint(&mut self) -> Result<Checkpoint> {
            Ok(if self.points.len() > 1 {
                self.points.pop_front().unwrap()
            } else {
                *self.points.front().unwrap()
            })
        }
        fn read_changes(&mut self, _: u64, _: i64) -> Result<Vec<u8>> {
            self.calls += 1;
            self.batches.pop_front().ok_or(Error::Changed)
        }
    }
    #[test]
    fn replay_deduplicates_references_and_preserves_changes_after_the_target() {
        let mut journal = Journal {
            points: VecDeque::from([point(0, 300)]),
            batches: VecDeque::from([
                batch(180, &[(1, 100), (1, 140)]),
                batch(300, &[(2, 180), (3, 250)]),
            ]),
            calls: 0,
        };
        let result = collect(&mut journal, point(0, 100), point(0, 200), || Ok(())).unwrap();
        assert_eq!(result.references, [FileReference(1), FileReference(2)]);
        assert_eq!(result.checkpoint.next, 200);
        assert_eq!(journal.calls, 2);
    }
    #[test]
    fn replacement_wrap_mid_read_and_stalled_cursors_never_publish_a_checkpoint() {
        let mut journal = Journal {
            points: VecDeque::from([point(0, 200), point(150, 200)]),
            batches: VecDeque::from([batch(200, &[(1, 100)])]),
            calls: 0,
        };
        assert!(matches!(
            collect(&mut journal, point(0, 100), point(0, 200), || Ok(())),
            Err(Error::JournalGap)
        ));
        let mut journal = Journal {
            points: VecDeque::from([Checkpoint {
                id: 8,
                ..point(0, 200)
            }]),
            batches: VecDeque::new(),
            calls: 0,
        };
        assert!(matches!(
            collect(&mut journal, point(0, 100), point(0, 200), || Ok(())),
            Err(Error::JournalGap)
        ));
        assert_eq!(journal.calls, 0);
        let mut journal = Journal {
            points: VecDeque::from([point(0, 200)]),
            batches: VecDeque::from([batch(100, &[])]),
            calls: 0,
        };
        assert!(matches!(
            collect(&mut journal, point(0, 100), point(0, 200), || Ok(())),
            Err(Error::Changed)
        ));
    }
    #[test]
    fn reject_unsupported_truncated_and_out_of_order_records() {
        let bytes = batch(200, &[(1, 100), (2, 150)]);
        for length in 0..bytes.len() {
            if length != 8 && length != 72 {
                assert!(decode_batch(&bytes[..length], 100, 200, &mut BTreeSet::new()).is_err());
            }
        }
        let mut bad = bytes.clone();
        bad[12] = 3;
        assert_eq!(
            decode_batch(&bad, 100, 200, &mut BTreeSet::new()).unwrap_err(),
            Error::Unsupported
        );
        let mut bad = bytes.clone();
        bad[8 + 58..8 + 60].copy_from_slice(&1000u16.to_le_bytes());
        assert_eq!(
            decode_batch(&bad, 100, 200, &mut BTreeSet::new()).unwrap_err(),
            Error::Corrupt
        );
        let bad = batch(200, &[(1, 150), (2, 100)]);
        assert_eq!(
            decode_batch(&bad, 100, 200, &mut BTreeSet::new()).unwrap_err(),
            Error::Corrupt
        );
        let mut journal = Journal {
            points: VecDeque::from([point(0, 200)]),
            batches: VecDeque::from([bytes]),
            calls: 0,
        };
        assert!(matches!(
            collect(&mut journal, point(0, 100), point(0, 200), || Err(
                Error::Cancelled
            )),
            Err(Error::Cancelled)
        ));
        assert_eq!(journal.calls, 0);
    }
}

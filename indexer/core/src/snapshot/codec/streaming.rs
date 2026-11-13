//! Verified, bounded-memory traversal of the unchanged metadata cache format.
//! Keep only file references, directory path fields and extension ownership; file
//! names are decoded as visited. The owned input MUST deny external mutation,
//! including deletion, for this reader's lifetime (as the native cache does).
use super::*;
use crate::DirectoryLookup;
use prism_ntfs_protocol::{DriveLetter, FileRecord};
use std::collections::BTreeMap;
mod directories;
mod replay;
mod writer;

pub struct Reader<R> {
    input: R,
    header: CacheHeader,
    references: Vec<FileReference>,
    directories: Catalog,
    base_directories: directories::Index,
    directory_records: usize,
    records_start: u64,
    records_bytes: u64,
    // Only replayed families are retained; None hides a deleted/reused cached
    // base. The reference index and serialized record region remain unchanged.
    overrides: BTreeMap<u64, Option<MetadataRecord>>,
    records: usize,
}

impl<R: Read + Seek> Reader<R> {
    /// Validate the full digest and every record/owner before any row visitor
    /// can run. Construction failure drops the private index and owned input.
    pub fn open(
        mut input: R,
        expected: Geometry,
        device: &str,
        check: impl Fn() -> super::super::layout::Result<()>,
    ) -> io::Result<Self> {
        let (mut body, header) = open_body(&mut input, expected, device, &check)?;
        let records_start = HEADER + header.length - body.limit();
        let mut references: Vec<FileReference> = Vec::with_capacity(header.records);
        let mut base_directories = directories::Index::default();
        let mut bytes = Vec::new();
        for index in 0..header.records {
            if index % 256 == 0 {
                checked(&check)?;
            }
            let record = read_record(&mut body, &mut bytes, decode_index_record)?;
            let number = record.reference.number();
            if references
                .last()
                .is_some_and(|last| last.number() >= number)
            {
                return Err(invalid());
            }
            references.push(record.reference);
            if record.is_dir {
                base_directories.push(&record)?;
            }
        }
        let records_bytes = HEADER + header.length - body.limit() - records_start;
        let reference = |number| {
            references
                .binary_search_by_key(&number, |reference| reference.number())
                .ok()
                .map(|index| references[index])
        };
        let (owners, families) = read_owners(&mut body, references.len(), reference, &check)?;
        checked(&check)?;
        if body.limit() != 0 {
            return Err(invalid());
        }
        let count = references.len();
        let directory_records = base_directories.len();
        Ok(Self {
            input,
            header,
            references,
            directories: Catalog {
                records: Records::default(),
                directories: HashMap::new(),
                owners,
                families,
            },
            records_start,
            records_bytes,
            overrides: BTreeMap::new(),
            records: count,
            base_directories,
            directory_records,
        })
    }

    pub fn geometry(&self) -> Geometry {
        self.header.geometry
    }
    pub fn checkpoint(&self) -> Checkpoint {
        self.header.checkpoint
    }
    pub fn record_count(&self) -> usize {
        self.records
    }
    pub fn directory_count(&self) -> usize {
        self.directory_records
    }
    fn is_directory(&self, number: u64) -> bool {
        directories::View {
            base: &self.base_directories,
            changes: &self.overrides,
        }
        .directory(number)
        .is_some()
    }
    pub fn reference(&self, number: u64) -> Option<FileReference> {
        if let Some(record) = self.overrides.get(&number) {
            return record.as_ref().map(|record| record.reference);
        }
        self.references
            .binary_search_by_key(&number, |reference| reference.number())
            .ok()
            .map(|index| self.references[index])
    }
    pub fn owner(&self, reference: FileReference) -> Option<FileReference> {
        self.directories.owner(reference)
    }
    pub fn family_members(&self, base: FileReference) -> &[u64] {
        self.directories
            .families
            .get(&base)
            .map_or(&[], Vec::as_slice)
    }

    /// Restart from the verified record region on every call, also after a
    /// visitor fails. At most one decoded unchanged file record is retained,
    /// in addition to the changed-family overlay from any successful refresh.
    pub fn visit_metadata(
        &mut self,
        check: impl Fn() -> super::super::layout::Result<()>,
        visit: impl FnMut(MetadataRecord) -> io::Result<()>,
    ) -> io::Result<()> {
        visit_merged(
            &mut self.input,
            &self.references,
            self.records_start,
            self.records_bytes,
            &self.overrides,
            &check,
            visit,
        )
    }

    /// Same path, hard-link, alias and sequence rules as Catalog::visit. This
    /// initially reflects the cached checkpoint. Call consuming refresh before
    /// publishing it as a current service snapshot. Neither operation enables
    /// early results during a cold MFT scan.
    pub fn visit(
        &mut self,
        drive: DriveLetter,
        check: impl Fn() -> super::super::layout::Result<()>,
        mut visit: impl FnMut(FileRecord) -> io::Result<()>,
    ) -> io::Result<()> {
        self.directories.directories.clear();
        let directories = &mut self.directories;
        let lookup = directories::View {
            base: &self.base_directories,
            changes: &self.overrides,
        };
        visit_merged(
            &mut self.input,
            &self.references,
            self.records_start,
            self.records_bytes,
            &self.overrides,
            &check,
            |record| {
                Catalog::visit_record(
                    &lookup,
                    &mut directories.directories,
                    drive,
                    record.reference.number(),
                    &record,
                    &mut visit,
                )
            },
        )
    }
}

fn visit_merged(
    input: &mut (impl Read + Seek),
    references: &[FileReference],
    start: u64,
    length: u64,
    overrides: &BTreeMap<u64, Option<MetadataRecord>>,
    check: &impl Fn() -> super::super::layout::Result<()>,
    mut visit: impl FnMut(MetadataRecord) -> io::Result<()>,
) -> io::Result<()> {
    let mut changed = overrides.iter().peekable();
    visit_records(input, references, start, length, check, |record| {
        let number = record.reference.number();
        while changed.peek().is_some_and(|(key, _)| **key < number) {
            checked(check)?;
            if let Some(record) = changed.next().unwrap().1 {
                visit(record.clone())?;
            }
        }
        if changed.peek().is_some_and(|(key, _)| **key == number) {
            if let Some(record) = changed.next().unwrap().1 {
                visit(record.clone())?;
            }
        } else {
            visit(record)?;
        }
        Ok(())
    })?;
    for (_, record) in changed {
        checked(check)?;
        if let Some(record) = record {
            visit(record.clone())?;
        }
    }
    checked(check)
}

fn read_record(
    body: &mut io::Take<impl Read>,
    bytes: &mut Vec<u8>,
    decode: impl FnOnce(&[u8]) -> io::Result<MetadataRecord>,
) -> io::Result<MetadataRecord> {
    let size = u32_value(body)? as usize;
    if !(38..=MAX_RECORD_BYTES).contains(&size) || size as u64 > body.limit() {
        return Err(invalid());
    }
    bytes.resize(size, 0);
    body.read_exact(bytes)?;
    decode(bytes)
}

fn visit_records(
    input: &mut (impl Read + Seek),
    references: &[FileReference],
    start: u64,
    length: u64,
    check: &impl Fn() -> super::super::layout::Result<()>,
    mut visit: impl FnMut(MetadataRecord) -> io::Result<()>,
) -> io::Result<()> {
    checked(check)?;
    input.seek(SeekFrom::Start(start))?;
    let mut body = input.take(length);
    let mut bytes = Vec::new();
    for (index, reference) in references.iter().enumerate() {
        if index % 256 == 0 {
            checked(check)?;
        }
        let record = read_record(&mut body, &mut bytes, decode_record)?;
        if record.reference != *reference {
            return Err(invalid());
        }
        visit(record)?;
    }
    checked(check)?;
    if body.limit() != 0 {
        return Err(invalid());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::tests::{encoded, rehash, sample, DEVICE};
    use super::*;
    use std::{cell::Cell, io::Cursor};
    fn paths(catalog: &mut Catalog, drive: DriveLetter) -> Vec<FileRecord> {
        let mut rows = vec![];
        catalog
            .visit(drive, |row| {
                rows.push(row);
                Ok::<_, ()>(())
            })
            .unwrap();
        rows
    }
    #[test]
    fn rows_and_extension_ownership_match_full_load_without_retaining_file_names() {
        let mut snapshot = sample();
        let bytes = encoded(&snapshot);
        let mut reader =
            Reader::open(Cursor::new(bytes), snapshot.geometry, DEVICE, || Ok(())).unwrap();
        assert_eq!(reader.record_count(), 2);
        assert_eq!(reader.directory_count(), 1);
        let base = FileReference((2 << 48) | 18);
        assert_eq!(reader.reference(18), Some(base));
        assert_eq!(reader.owner(FileReference((3 << 48) | 42)), Some(base));
        assert_eq!(reader.family_members(base), &[42]);
        let actual = reader.geometry();
        let expected = snapshot.geometry;
        assert_eq!(
            (
                actual.sector_size,
                actual.cluster_size,
                actual.record_size,
                actual.volume_bytes,
                actual.mft_bytes,
                actual.serial
            ),
            (
                expected.sector_size,
                expected.cluster_size,
                expected.record_size,
                expected.volume_bytes,
                expected.mft_bytes,
                expected.serial
            )
        );
        assert_eq!(reader.checkpoint(), snapshot.checkpoint);
        for letter in *b"CDC" {
            let drive = DriveLetter::new(letter).unwrap();
            let mut actual = vec![];
            reader
                .visit(
                    drive,
                    || Ok(()),
                    |row| {
                        actual.push(row);
                        Ok(())
                    },
                )
                .unwrap();
            assert_eq!(actual, paths(&mut snapshot.catalog, drive));
        }
    }
    #[test]
    fn corruptions_have_the_same_acceptance_as_full_load_before_visiting() {
        let snapshot = sample();
        let original = encoded(&snapshot);
        for index in 0..original.len() {
            let mut bytes = original.clone();
            bytes[index] ^= 0x81;
            if index >= HEADER as usize {
                rehash(&mut bytes);
            }
            let full = read(
                &mut Cursor::new(bytes.clone()),
                snapshot.geometry,
                DEVICE,
                || Ok(()),
            );
            let streaming = Reader::open(Cursor::new(bytes), snapshot.geometry, DEVICE, || Ok(()));
            assert_eq!(full.is_ok(), streaming.is_ok(), "mutation at {index}");
        }
    }
    #[test]
    fn cancellation_and_callback_failure_stop_and_can_restart_the_owned_reader() {
        let snapshot = sample();
        let bytes = encoded(&snapshot);
        for limit in 0..7 {
            let count = Cell::new(0);
            let check = || {
                let n = count.get();
                count.set(n + 1);
                if n >= limit {
                    Err(crate::layout::Error::Cancelled)
                } else {
                    Ok(())
                }
            };
            let result = Reader::open(Cursor::new(bytes.clone()), snapshot.geometry, DEVICE, check);
            if limit < 5 {
                assert!(result.is_err());
            }
        }
        let mut reader =
            Reader::open(Cursor::new(bytes), snapshot.geometry, DEVICE, || Ok(())).unwrap();
        let drive = DriveLetter::new(b'C').unwrap();
        let mut count = 0;
        assert!(reader
            .visit(
                drive,
                || Ok(()),
                |_| {
                    count += 1;
                    Err(io::ErrorKind::Interrupted.into())
                }
            )
            .is_err());
        assert_eq!(count, 1);
        let mut rows = vec![];
        reader
            .visit(
                drive,
                || Ok(()),
                |row| {
                    rows.push(row);
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(rows.len(), 3);
        assert!(reader
            .visit(
                drive,
                || Err(crate::layout::Error::Cancelled),
                |_| panic!("cancelled visitor")
            )
            .is_err());
    }
}

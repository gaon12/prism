//! Row-distribution fixture, not original MFT IDs/ownership/geometry/journal.
use super::*;
use prism_ntfs_core::{Catalog, MetadataRecord};
use std::{
    collections::HashMap,
    io::{Seek, SeekFrom},
    time::Duration,
};

struct Directory {
    reference: FileReference,
    received: bool,
}
struct Builder {
    catalog: CatalogBuilder,
    directories: HashMap<String, Directory>,
    template: MetadataRecord,
    drive: Option<DriveLetter>,
    next: u64,
    nodes: usize,
}
fn invalid() -> io::Error {
    io::ErrorKind::InvalidData.into()
}
fn parent_name(path: &str) -> io::Result<(&str, &str)> {
    let split = path.rfind('\\').ok_or_else(invalid)?;
    let parent = if split == 2 {
        &path[..3]
    } else {
        &path[..split]
    };
    let name = &path[split + 1..];
    if name.is_empty() || name.encode_utf16().count() > 255 {
        return Err(invalid());
    }
    Ok((parent, name))
}
impl Builder {
    fn new() -> io::Result<Self> {
        let mut bytes = Vec::new();
        GzDecoder::new(&include_bytes!("../../tests/fixtures/testfs1-mft.gz")[..])
            .take(3 * 1024 * 1024)
            .read_to_end(&mut bytes)?;
        let mut template = parse_record(65, 512, &bytes[65 * 1024..66 * 1024])
            .map_err(metadata_error)?
            .ok_or_else(invalid)?;
        template.names.clear();
        Ok(Self {
            catalog: CatalogBuilder::default(),
            directories: HashMap::new(),
            template,
            drive: None,
            next: 16,
            nodes: 0,
        })
    }
    fn allocate(&mut self) -> io::Result<FileReference> {
        if self.nodes >= 4_000_000 {
            return Err(invalid());
        }
        let value = reference(self.next);
        self.next += 1;
        self.nodes += 1;
        Ok(value)
    }
    fn record(
        &self,
        id: FileReference,
        parent: Option<(FileReference, &str)>,
        is_dir: bool,
        size: u64,
        modified: Option<i64>,
    ) -> MetadataRecord {
        let mut record = self.template.clone();
        record.reference = id;
        record.base = None;
        record.is_dir = is_dir;
        record.size = Some(size);
        record.modified_ms = modified;
        record.names = parent.map_or_else(Vec::new, |(parent, name)| {
            vec![FileName {
                parent,
                name: name.to_owned(),
                namespace: 1,
            }]
        });
        record
    }
    fn initialize(&mut self, drive: DriveLetter) -> io::Result<()> {
        let root = self.record(reference(5), None, true, 0, None);
        self.catalog.insert(root).map_err(metadata_error)?;
        self.directories.insert(
            drive.root(),
            Directory {
                reference: reference(5),
                received: false,
            },
        );
        self.drive = Some(drive);
        self.nodes = 1;
        Ok(())
    }
    fn directory(&mut self, path: &str) -> io::Result<FileReference> {
        let mut current = path;
        let mut pending = Vec::new();
        let mut parent = loop {
            if let Some(directory) = self.directories.get(current) {
                break directory.reference;
            }
            let (above, name) = parent_name(current)?;
            pending.push((current, name));
            current = above;
        };
        for (path, name) in pending.into_iter().rev() {
            let id = self.allocate()?;
            self.catalog
                .insert(self.record(id, Some((parent, name)), true, 0, None))
                .map_err(metadata_error)?;
            self.directories.insert(
                path.to_owned(),
                Directory {
                    reference: id,
                    received: false,
                },
            );
            parent = id;
        }
        Ok(parent)
    }
    fn push(&mut self, row: FileRecord) -> io::Result<()> {
        let drive = DriveLetter::new(*row.path.as_bytes().first().ok_or_else(invalid)?)?;
        if !drive.contains(&row.path) || row.path.as_bytes()[0] != drive.byte() {
            return Err(invalid());
        }
        if let Some(old) = self.drive {
            if old != drive {
                return Err(invalid());
            }
        } else {
            self.initialize(drive)?;
        }
        if row.is_dir {
            if row.size != 0 {
                return Err(invalid());
            }
            let mut id = self.directory(&row.path)?;
            let directory = self.directories.get_mut(&row.path).unwrap();
            if directory.received {
                if row.path.len() == 3 {
                    return Err(invalid());
                }
                // Preserve repeated visible directory rows without changing
                // the canonical parent used to reconstruct child paths.
                id = self.allocate()?;
            } else {
                directory.received = true;
            }
            let parent = if row.path.len() == 3 {
                None
            } else {
                let (above, name) = parent_name(&row.path)?;
                Some((
                    self.directories.get(above).ok_or_else(invalid)?.reference,
                    name,
                ))
            };
            self.catalog
                .insert(self.record(id, parent, true, 0, row.modified_ms))
                .map_err(metadata_error)?;
        } else {
            if row.path.len() == 3 || self.directories.contains_key(&row.path) {
                return Err(invalid());
            }
            let (above, name) = parent_name(&row.path)?;
            let parent = self.directory(above)?;
            let id = self.allocate()?;
            self.catalog
                .insert(self.record(id, Some((parent, name)), false, row.size, row.modified_ms))
                .map_err(metadata_error)?;
        }
        Ok(())
    }
    fn finish(self, drive: DriveLetter) -> io::Result<Catalog> {
        if self.drive != Some(drive)
            || self
                .directories
                .values()
                .any(|directory| !directory.received)
        {
            return Err(invalid());
        }
        self.catalog.finish().map_err(metadata_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn row(path: &str, directory: bool, size: u64, time: Option<i64>) -> FileRecord {
        FileRecord {
            path: path.into(),
            is_dir: directory,
            size,
            modified_ms: time,
        }
    }
    fn sort(rows: &mut [FileRecord]) {
        rows.sort_by(|a, b| {
            (&a.path, a.is_dir, a.size, a.modified_ms).cmp(&(
                &b.path,
                b.is_dir,
                b.size,
                b.modified_ms,
            ))
        });
    }
    #[test]
    fn parents_arriving_late_case_unicode_and_repeated_rows_preserve_every_field() {
        let mut expected = vec![
            row(r"C:\docs\자료-😀.md", false, 17, Some(i64::MIN)),
            row(r"C:\DOCS\別名.md", false, 0, None),
            row(r"C:\docs\자료-😀.md", false, 17, Some(i64::MIN)),
            row(r"C:\", true, 0, None),
            row(r"C:\docs", true, 0, Some(17)),
            row(r"C:\DOCS", true, 0, Some(18)),
            row(r"C:\docs", true, 0, Some(19)),
        ];
        let mut builder = Builder::new().unwrap();
        for value in expected.clone() {
            builder.push(value).unwrap();
        }
        let mut catalog = builder.finish(DriveLetter::new(b'C').unwrap()).unwrap();
        assert_eq!(catalog.record_count(), expected.len());
        let mut actual = Vec::new();
        catalog
            .visit(DriveLetter::new(b'C').unwrap(), |row| {
                actual.push(row);
                Ok::<_, ()>(())
            })
            .unwrap();
        let left = multiset(expected.iter().map(fingerprint).collect());
        let right = multiset(actual.iter().map(fingerprint).collect());
        assert_eq!(left, right);
        sort(&mut expected);
        sort(&mut actual);
        assert_eq!(actual, expected);
    }
    #[test]
    fn incomplete_parent_metadata_bad_root_and_out_of_scope_paths_are_rejected() {
        let mut builder = Builder::new().unwrap();
        builder
            .push(row(r"C:\missing\a.md", false, 1, None))
            .unwrap();
        builder.push(row(r"C:\", true, 0, None)).unwrap();
        assert!(builder.finish(DriveLetter::new(b'C').unwrap()).is_err());
        for value in [
            row(r"C:\", false, 0, None),
            row(r"C:\dir", true, 1, None),
            row(r"C:\..\a", false, 0, None),
            row(r"c:\lower-drive", false, 0, None),
            row(&format!("C:\\{}", "😀".repeat(128)), false, 0, None),
        ] {
            assert!(Builder::new().unwrap().push(value).is_err());
        }
        let mut builder = Builder::new().unwrap();
        builder.push(row(r"C:\", true, 0, None)).unwrap();
        assert!(builder.push(row(r"C:\", true, 0, None)).is_err());
        assert!(builder.push(row(r"D:\", true, 0, None)).is_err());
    }
    #[test]
    fn row_multiset_preserves_duplicate_multiplicity_and_optional_time() {
        let a = row(r"C:\a", false, 0, None);
        let b = row(r"C:\a", false, 0, Some(0));
        assert_ne!(fingerprint(&a), fingerprint(&b));
        assert_ne!(
            multiset(vec![fingerprint(&a)]),
            multiset(vec![fingerprint(&a), fingerprint(&a)])
        );
        assert_eq!(
            multiset(vec![fingerprint(&a), fingerprint(&b)]),
            multiset(vec![fingerprint(&b), fingerprint(&a)])
        );
    }
}
fn fingerprint(row: &FileRecord) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update((row.path.len() as u64).to_le_bytes());
    digest.update(row.path.as_bytes());
    digest.update([u8::from(row.is_dir)]);
    digest.update(row.size.to_le_bytes());
    digest.update([u8::from(row.modified_ms.is_some())]);
    digest.update(row.modified_ms.unwrap_or(0).to_le_bytes());
    digest.finalize().into()
}
fn multiset(mut rows: Vec<[u8; 32]>) -> String {
    rows.sort_unstable();
    let mut digest = Sha256::new();
    digest.update(b"Prism row multiset v1");
    digest.update((rows.len() as u64).to_le_bytes());
    for row in rows {
        digest.update(row);
    }
    format!("{:x}", digest.finalize())
}
pub(super) fn run(path: &Path, output_path: &Path) -> Result<(), Box<dyn Error>> {
    start_gate()?;
    let started = Instant::now();
    let check = || {
        if started.elapsed() > Duration::from_secs(180) {
            Err(io::ErrorKind::TimedOut.into())
        } else {
            Ok(())
        }
    };
    let mut input = BufReader::with_capacity(1024 * 1024, open_immutable(path)?);
    let mut builder = Builder::new()?;
    let summary = captured_rows::read(&mut input, check, |row| builder.push(row))?;
    let catalog = builder.finish(summary.drive)?;
    let metadata_records = catalog.record_count();
    let snapshot = Snapshot {
        catalog,
        checkpoint: Checkpoint {
            id: 99,
            first: 0,
            next: 128,
            lowest: 0,
        },
        records_read: metadata_records as u64,
        geometry: geometry(),
    };
    let mut output = BufWriter::with_capacity(
        1024 * 1024,
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(output_path)?,
    );
    codec::write(&snapshot, DEVICE, &mut output, || {
        check().map_err(prism_ntfs_core::layout::Error::from)
    })?;
    output.flush()?;
    output.get_ref().sync_all()?;
    drop(output);
    drop(snapshot);
    let written = started.elapsed().as_secs_f64();
    input.seek(SeekFrom::Start(0))?;
    let mut expected = Vec::with_capacity(summary.records as usize);
    let repeated = captured_rows::read(&mut input, check, |row| {
        expected.push(fingerprint(&row));
        Ok(())
    })?;
    if repeated != summary {
        return Err(invalid().into());
    }
    let expected = multiset(expected);
    drop(input);
    let mut actual = Vec::with_capacity(summary.records as usize);
    let mut cache = codec::streaming::Reader::open(
        BufReader::with_capacity(1024 * 1024, open_immutable(output_path)?),
        geometry(),
        DEVICE,
        || check().map_err(prism_ntfs_core::layout::Error::from),
    )?;
    cache.visit(
        summary.drive,
        || check().map_err(prism_ntfs_core::layout::Error::from),
        |row| {
            actual.push(fingerprint(&row));
            Ok(())
        },
    )?;
    if actual.len() as u64 != summary.records || multiset(actual) != expected {
        return Err(invalid().into());
    }
    println!("{{\"mode\":\"import\",\"derivedFromCapturedRows\":true,\"syntheticReferencesGeometryCheckpoint\":true,\"startGateUsed\":true,\"metadataRecords\":{metadata_records},\"records\":{},\"pathBytes\":{},\"capturedRevisionIsNotUsn\":true,\"writeSeconds\":{written},\"totalSeconds\":{},\"rowMultisetSha256\":\"{expected}\",\"multisetVerified\":true}}",summary.records,summary.path_bytes,started.elapsed().as_secs_f64());
    Ok(())
}

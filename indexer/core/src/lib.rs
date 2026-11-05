//! Read-only NTFS metadata parsing, independent of service privileges and I/O.
//! This module never opens a file, reads user data streams, or writes a volume.
//! The Windows adapter must obtain a consistent MFT stream and replay the USN
//! journal before publishing a snapshot built from these records.

use prism_ntfs_protocol::{DriveLetter, FileRecord};
use std::collections::{HashMap, HashSet};
pub mod journal;
pub mod layout;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FileReference(pub u64);
impl FileReference {
    pub fn number(self) -> u64 {
        self.0 & 0x0000_ffff_ffff_ffff
    }
    pub fn sequence(self) -> u16 {
        (self.0 >> 48) as u16
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParseError {
    Bounds,
    Signature,
    Fixup,
    Attribute,
    Name,
    Limit,
}
type Result<T> = std::result::Result<T, ParseError>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileName {
    pub parent: FileReference,
    pub name: String,
    pub namespace: u8,
}

#[derive(Clone, Debug)]
pub struct MetadataRecord {
    pub reference: FileReference,
    pub base: Option<FileReference>,
    pub is_dir: bool,
    pub names: Vec<FileName>,
    pub size: Option<u64>,
    pub modified_ms: Option<i64>,
    attributes: u32,
    reparse_tag: Option<u32>,
}
impl MetadataRecord {
    fn alias(&self) -> bool {
        // Name-surrogate reparses (including junctions and symlinks) redirect
        // path resolution. Cloud placeholders have no such bit and can supply
        // filenames without downloading their contents. Unknown tags fail closed.
        self.attributes & 0x400 != 0 && self.reparse_tag.is_none_or(|tag| tag & 0x2000_0000 != 0)
    }
}

fn slice(bytes: &[u8], offset: usize, length: usize) -> Result<&[u8]> {
    bytes
        .get(offset..offset.checked_add(length).ok_or(ParseError::Bounds)?)
        .ok_or(ParseError::Bounds)
}
fn u16_at(bytes: &[u8], offset: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(
        slice(bytes, offset, 2)?.try_into().unwrap(),
    ))
}
fn u32_at(bytes: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(
        slice(bytes, offset, 4)?.try_into().unwrap(),
    ))
}
fn u64_at(bytes: &[u8], offset: usize) -> Result<u64> {
    Ok(u64::from_le_bytes(
        slice(bytes, offset, 8)?.try_into().unwrap(),
    ))
}
fn filetime_ms(value: u64) -> Result<i64> {
    i64::try_from(i128::from(value / 10_000) - 11_644_473_600_000i128)
        .map_err(|_| ParseError::Attribute)
}

/// Decode one on-disk FILE record, including its update-sequence fixups.
/// Deleted and zero-filled records are ignored. A bad live record is returned
/// as an error, never converted into an invented filename or timestamp.
pub fn parse_record(
    number: u64,
    sector_size: usize,
    bytes: &[u8],
) -> Result<Option<MetadataRecord>> {
    if number > 0x0000_ffff_ffff_ffff {
        return Err(ParseError::Bounds);
    }
    let Some(bytes) = decoded_record(bytes, sector_size, false)? else {
        return Ok(None);
    };
    parse_decoded(number, &bytes).map(Some)
}

/// Kernel IOCTL buffers can already have their sector fixups applied. This
/// entry point must only receive FSCTL_GET_NTFS_FILE_RECORD output. Raw volume
/// reads always use `parse_record`, which requires intact on-disk markers.
pub fn parse_kernel_record(
    number: u64,
    sector_size: usize,
    bytes: &[u8],
) -> Result<Option<MetadataRecord>> {
    if number > 0x0000_ffff_ffff_ffff {
        return Err(ParseError::Bounds);
    }
    let Some(bytes) = decoded_record(bytes, sector_size, true)? else {
        return Ok(None);
    };
    parse_decoded(number, &bytes).map(Some)
}

pub(crate) fn decoded_record(
    bytes: &[u8],
    sector_size: usize,
    kernel: bool,
) -> Result<Option<Vec<u8>>> {
    if bytes.len() < 48
        || bytes.len() > 65536
        || !sector_size.is_power_of_two()
        || !(512..=4096).contains(&sector_size)
        || !bytes.len().is_multiple_of(sector_size)
    {
        return Err(ParseError::Bounds);
    }
    if bytes.iter().all(|byte| *byte == 0) {
        return Ok(None);
    }
    if &bytes[..4] != b"FILE" {
        return Err(ParseError::Signature);
    }
    if u16_at(bytes, 22)? & 1 == 0 {
        return Ok(None);
    }
    let used = u32_at(bytes, 24)? as usize;
    let allocated = u32_at(bytes, 28)? as usize;
    let first = u16_at(bytes, 20)? as usize;
    let usa = u16_at(bytes, 4)? as usize;
    let usa_count = u16_at(bytes, 6)? as usize;
    if used > bytes.len()
        || allocated > bytes.len()
        || used > allocated
        || first < 48
        || first >= used
        || !first.is_multiple_of(8)
        || usa < 8
        || usa_count != bytes.len() / sector_size + 1
        || usa.checked_add(usa_count * 2).is_none_or(|end| end > first)
    {
        return Err(ParseError::Bounds);
    }
    let mut fixed = bytes.to_vec();
    let marker = u16_at(bytes, usa)?;
    let protected =
        (1..usa_count).all(|sector| u16_at(bytes, sector * sector_size - 2) == Ok(marker));
    let applied = kernel
        && (1..usa_count).all(|sector| {
            u16_at(bytes, sector * sector_size - 2) == u16_at(bytes, usa + sector * 2)
        });
    if !protected && !applied {
        return Err(ParseError::Fixup);
    }
    for sector in 1..usa_count {
        let end = sector * sector_size - 2;
        if protected {
            fixed[end..end + 2].copy_from_slice(slice(bytes, usa + sector * 2, 2)?);
        }
    }
    fixed.truncate(used);
    Ok(Some(fixed))
}

fn parse_decoded(number: u64, bytes: &[u8]) -> Result<MetadataRecord> {
    let first = u16_at(bytes, 20)? as usize;
    let base = FileReference(u64_at(bytes, 32)?);
    let mut record = MetadataRecord {
        reference: FileReference(number | (u64::from(u16_at(bytes, 16)?) << 48)),
        base: (base.0 != 0).then_some(base),
        is_dir: u16_at(bytes, 22)? & 2 != 0,
        names: vec![],
        size: None,
        modified_ms: None,
        attributes: 0,
        reparse_tag: None,
    };
    let mut offset = first;
    loop {
        let kind = u32_at(bytes, offset)?;
        if kind == u32::MAX {
            break;
        }
        let length = u32_at(bytes, offset + 4)? as usize;
        if length < 24 || !length.is_multiple_of(8) {
            return Err(ParseError::Attribute);
        }
        let attribute = slice(bytes, offset, length)?;
        let nonresident = attribute[8];
        let named = attribute[9] != 0;
        if nonresident > 1 {
            return Err(ParseError::Attribute);
        }
        if named {
            let name_offset = u16_at(attribute, 10)? as usize;
            if name_offset < 24 {
                return Err(ParseError::Attribute);
            }
            slice(attribute, name_offset, usize::from(attribute[9]) * 2)?;
        }
        if nonresident == 0 {
            let value_len = u32_at(attribute, 16)? as usize;
            let value_offset = u16_at(attribute, 20)? as usize;
            if value_offset < 24 {
                return Err(ParseError::Attribute);
            }
            let value = slice(attribute, value_offset, value_len)?;
            match kind {
                0x10 if !named => {
                    record.modified_ms = Some(filetime_ms(u64_at(value, 8)?)?);
                    record.attributes = u32_at(value, 32)?;
                }
                0x30 if !named => {
                    if record.names.len() >= 4096 {
                        return Err(ParseError::Limit);
                    }
                    let length = usize::from(*value.get(64).ok_or(ParseError::Name)?);
                    let namespace = *value.get(65).ok_or(ParseError::Name)?;
                    if namespace > 3 || length == 0 {
                        return Err(ParseError::Name);
                    }
                    let encoded = slice(value, 66, length * 2)?;
                    let units: Vec<_> = encoded
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|unit| u16::from_le_bytes(*unit))
                        .collect();
                    let name = String::from_utf16(&units).map_err(|_| ParseError::Name)?;
                    if name.contains(['\0', '\\', '/', ':']) {
                        return Err(ParseError::Name);
                    }
                    // Root record 5 has the special name "."; it is constructed
                    // as a drive root rather than joined to a parent below.
                    if name != "." && name != ".." {
                        record.names.push(FileName {
                            parent: FileReference(u64_at(value, 0)?),
                            name,
                            namespace,
                        });
                    }
                    record.attributes |= u32_at(value, 56)? & 0x400;
                }
                0x80 if !named => record.size = Some(value_len as u64),
                0xc0 if !named => record.reparse_tag = Some(u32_at(value, 0)?),
                _ => {}
            }
        } else {
            if length < 64 {
                return Err(ParseError::Attribute);
            }
            // A named $DATA attribute is an alternate stream. Its size and
            // contents have no place in the filename index.
            if kind == 0x80 && !named && u64_at(attribute, 16)? == 0 {
                record.size = Some(u64_at(attribute, 48)?);
            }
        }
        offset = offset.checked_add(length).ok_or(ParseError::Bounds)?;
    }
    if record.is_dir {
        record.size = Some(0);
    }
    Ok(record)
}

/// Builds path metadata from the complete MFT, not a recursive directory walk.
/// Extension records are joined only to the base record with the same sequence
/// number. Parent references are checked the same way to prevent handle reuse.
#[derive(Default)]
pub struct CatalogBuilder {
    records: HashMap<u64, MetadataRecord>,
    extensions: Vec<MetadataRecord>,
}
impl CatalogBuilder {
    pub fn insert(&mut self, record: MetadataRecord) -> Result<()> {
        if self.records.len() + self.extensions.len() >= 10_000_000 {
            return Err(ParseError::Limit);
        }
        if record.base.is_some() {
            self.extensions.push(record);
        } else {
            self.records.insert(record.reference.number(), record);
        }
        Ok(())
    }
    pub fn finish(mut self) -> Result<Catalog> {
        for extension in self.extensions {
            let base = extension.base.unwrap();
            if let Some(record) = self
                .records
                .get_mut(&base.number())
                .filter(|record| record.reference == base)
            {
                if record.names.len() + extension.names.len() > 4096 {
                    return Err(ParseError::Limit);
                }
                record.names.extend(extension.names);
                if record.size.is_none() {
                    record.size = extension.size;
                }
                if record.modified_ms.is_none() {
                    record.modified_ms = extension.modified_ms;
                }
                record.attributes |= extension.attributes;
                if record.reparse_tag.is_none() {
                    record.reparse_tag = extension.reparse_tag;
                }
            }
        }
        for record in self.records.values_mut() {
            if record.names.iter().any(|name| name.namespace != 2) {
                record.names.retain(|name| name.namespace != 2);
            }
            let mut seen = HashSet::new();
            record
                .names
                .retain(|name| seen.insert((name.parent, name.name.clone())));
        }
        Ok(Catalog {
            records: self.records,
            directories: HashMap::new(),
        })
    }
}

pub struct Catalog {
    records: HashMap<u64, MetadataRecord>,
    directories: HashMap<FileReference, Option<String>>,
}
impl Catalog {
    fn parent_path(&mut self, drive: DriveLetter, reference: FileReference) -> Option<String> {
        let mut current = reference;
        let mut chain = vec![];
        let mut seen = HashSet::new();
        let mut path =
            loop {
                if let Some(path) = self.directories.get(&current) {
                    break path.clone();
                }
                if !seen.insert(current) || chain.len() > 16384 {
                    break None;
                }
                let Some(record) = self.records.get(&current.number()).filter(|record| {
                    record.reference == current && record.is_dir && !record.alias()
                }) else {
                    break None;
                };
                if current.number() == 5 {
                    break Some(drive.root());
                }
                if current.number() < 16 {
                    break None;
                }
                let Some(name) = record.names.first() else {
                    break None;
                };
                chain.push((current, name.name.clone()));
                current = name.parent;
            };
        for (reference, name) in chain.into_iter().rev() {
            path = path.and_then(|parent| {
                let next = join_path(&parent, &name);
                drive.contains(&next).then_some(next)
            });
            self.directories.insert(reference, path.clone());
        }
        path
    }
    /// Records are visited in deterministic MFT-number order. Callback failure
    /// (including cancellation or a closed pipe) stops without finishing a
    /// partial snapshot. The adapter must send Finished only after success.
    pub fn visit<E>(
        &mut self,
        drive: DriveLetter,
        mut visit: impl FnMut(FileRecord) -> std::result::Result<(), E>,
    ) -> std::result::Result<(), E> {
        // Directory paths are drive-specific. A catalog may be reused after
        // Windows assigns another drive letter to the same volume.
        self.directories.clear();
        let mut numbers: Vec<_> = self.records.keys().copied().collect();
        numbers.sort_unstable();
        for number in numbers {
            let record = &self.records[&number];
            if record.base.is_some() || record.alias() || (number < 16 && number != 5) {
                continue;
            }
            if number == 5 {
                visit(FileRecord {
                    path: drive.root(),
                    is_dir: true,
                    size: 0,
                    modified_ms: record.modified_ms,
                })?;
                continue;
            }
            let is_dir = record.is_dir;
            let Some(size) = record.size.or_else(|| record.is_dir.then_some(0)) else {
                continue;
            };
            let modified_ms = record.modified_ms;
            let names = record.names.clone();
            for name in names {
                if let Some(parent) = self.parent_path(drive, name.parent) {
                    let path = join_path(&parent, &name.name);
                    if drive.contains(&path) {
                        visit(FileRecord {
                            path,
                            is_dir,
                            size,
                            modified_ms,
                        })?;
                    }
                }
            }
        }
        Ok(())
    }
}
fn join_path(parent: &str, name: &str) -> String {
    if parent.ends_with('\\') {
        format!("{parent}{name}")
    } else {
        format!("{parent}\\{name}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn reference(number: u64, sequence: u16) -> FileReference {
        FileReference(number | (u64::from(sequence) << 48))
    }
    fn record(number: u64, parent: u64, name: &str, is_dir: bool) -> MetadataRecord {
        MetadataRecord {
            reference: reference(number, 1),
            base: None,
            is_dir,
            names: vec![FileName {
                parent: reference(parent, 1),
                name: name.into(),
                namespace: 1,
            }],
            size: Some(if is_dir { 0 } else { 123 }),
            modified_ms: Some(42),
            attributes: 0,
            reparse_tag: None,
        }
    }
    fn catalog(records: Vec<MetadataRecord>) -> Catalog {
        let mut builder = CatalogBuilder::default();
        for record in records {
            builder.insert(record).unwrap();
        }
        builder.finish().unwrap()
    }
    fn paths(catalog: &mut Catalog, drive: u8) -> Vec<String> {
        let mut paths = vec![];
        catalog
            .visit(DriveLetter::new(drive).unwrap(), |file| {
                paths.push(file.path);
                Ok::<_, ()>(())
            })
            .unwrap();
        paths
    }
    #[test]
    fn paths_preserve_hard_links_and_reject_reused_parent_references() {
        let mut linked = record(18, 16, "파일.md", false);
        linked.names.push(FileName {
            parent: reference(17, 1),
            name: "다른 이름.md".into(),
            namespace: 1,
        });
        linked.names.push(FileName {
            parent: reference(16, 1),
            name: "FILE~1.MD".into(),
            namespace: 2,
        });
        let mut stale = record(19, 16, "stale.md", false);
        stale.names[0].parent = reference(16, 2);
        let mut catalog = catalog(vec![
            record(5, 5, ".", true),
            record(16, 5, "문서", true),
            record(17, 5, "참조", true),
            linked,
            stale,
        ]);
        assert_eq!(
            paths(&mut catalog, b'C'),
            [
                "C:\\",
                "C:\\문서",
                "C:\\참조",
                "C:\\문서\\파일.md",
                "C:\\참조\\다른 이름.md"
            ]
        );
        assert!(paths(&mut catalog, b'D')
            .iter()
            .all(|path| path.starts_with("D:\\")));
    }
    #[test]
    fn extension_names_require_the_correct_base_sequence() {
        let mut extension = record(20, 5, "hardlink.md", false);
        extension.base = Some(reference(18, 1));
        let mut stale = record(21, 5, "wrong.md", false);
        stale.base = Some(reference(18, 2));
        let mut catalog = catalog(vec![
            record(5, 5, ".", true),
            record(18, 5, "original.md", false),
            extension,
            stale,
        ]);
        assert_eq!(
            paths(&mut catalog, b'C'),
            ["C:\\", "C:\\original.md", "C:\\hardlink.md"]
        );
    }
    #[test]
    fn junctions_cycles_and_orphans_do_not_escape_the_volume() {
        let mut junction = record(16, 5, "redirect", true);
        junction.attributes = 0x400;
        junction.reparse_tag = Some(0xa0000003);
        let mut cloud = record(20, 5, "cloud", true);
        cloud.attributes = 0x400;
        cloud.reparse_tag = Some(0x9000001a);
        let mut catalog = catalog(vec![
            record(5, 5, ".", true),
            junction,
            record(17, 16, "private.md", false),
            record(18, 19, "cycle-a", true),
            record(19, 18, "cycle-b", true),
            cloud,
            record(21, 20, "placeholder.md", false),
            record(22, 99, "orphan.md", false),
        ]);
        assert_eq!(
            paths(&mut catalog, b'C'),
            ["C:\\", "C:\\cloud", "C:\\cloud\\placeholder.md"]
        );
        let mut count = 0;
        assert!(catalog
            .visit(DriveLetter::new(b'C').unwrap(), |_| {
                count += 1;
                if count == 2 {
                    Err(())
                } else {
                    Ok(())
                }
            })
            .is_err());
        assert_eq!(count, 2);
    }
    fn put16(bytes: &mut [u8], offset: usize, value: u16) {
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }
    fn put32(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    fn put64(bytes: &mut [u8], offset: usize, value: u64) {
        bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }
    fn resident(kind: u32, value: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0; (24 + value.len()).next_multiple_of(8)];
        let len = bytes.len();
        put32(&mut bytes, 0, kind);
        put32(&mut bytes, 4, len as u32);
        put32(&mut bytes, 16, value.len() as u32);
        put16(&mut bytes, 20, 24);
        bytes[24..24 + value.len()].copy_from_slice(value);
        bytes
    }
    fn encoded(attributes: Vec<Vec<u8>>) -> Vec<u8> {
        let mut bytes = vec![0; 1024];
        bytes[..4].copy_from_slice(b"FILE");
        put16(&mut bytes, 4, 48);
        put16(&mut bytes, 6, 3);
        put16(&mut bytes, 16, 7);
        put16(&mut bytes, 20, 56);
        put16(&mut bytes, 22, 1);
        put32(&mut bytes, 28, 1024);
        let mut offset = 56;
        for attribute in attributes {
            bytes[offset..offset + attribute.len()].copy_from_slice(&attribute);
            offset += attribute.len();
        }
        put32(&mut bytes, offset, u32::MAX);
        put32(&mut bytes, 24, (offset + 4) as u32);
        put16(&mut bytes, 48, 0xbeef);
        for (sector, tail) in [(1, 510), (2, 1022)] {
            let old = u16_at(&bytes, tail).unwrap();
            put16(&mut bytes, 48 + sector * 2, old);
            put16(&mut bytes, tail, 0xbeef);
        }
        bytes
    }
    #[test]
    fn on_disk_parser_reads_real_size_and_standard_modified_time_only() {
        let mut standard = vec![0; 48];
        put64(&mut standard, 8, 11644473600000 * 10000 + 42 * 10000);
        let mut name = vec![0; 66 + 4];
        put64(&mut name, 0, reference(5, 1).0);
        put64(&mut name, 48, 999);
        name[64] = 2;
        name[65] = 1;
        name[66..70].copy_from_slice(&[b'a', 0, b'b', 0]);
        let bytes = encoded(vec![
            resident(0x10, &standard),
            resident(0x30, &name),
            resident(0x80, b"12345"),
        ]);
        let parsed = parse_record(18, 512, &bytes).unwrap().unwrap();
        assert_eq!(parsed.reference, reference(18, 7));
        assert_eq!(parsed.size, Some(5));
        assert_eq!(parsed.modified_ms, Some(42));
        assert_eq!(parsed.names[0].name, "ab");
        let mut torn = bytes.clone();
        torn[510] = 0;
        assert_eq!(parse_record(18, 512, &torn).unwrap_err(), ParseError::Fixup);
        let mut truncated = bytes.clone();
        put32(&mut truncated, 24, 2048);
        assert_eq!(
            parse_record(18, 512, &truncated).unwrap_err(),
            ParseError::Bounds
        );
        let mut deleted = bytes;
        put16(&mut deleted, 22, 0);
        assert!(parse_record(18, 512, &deleted).unwrap().is_none());
        assert!(parse_record(18, 512, &[0; 1024]).unwrap().is_none());
    }
    #[test]
    fn fixups_restore_a_name_crossing_a_sector_and_nonresident_file_size() {
        let text = "한".repeat(200);
        let mut name = vec![0; 66 + 400];
        put64(&mut name, 0, reference(5, 1).0);
        name[64] = 200;
        name[65] = 1;
        for (index, unit) in text.encode_utf16().enumerate() {
            put16(&mut name, 66 + index * 2, unit);
        }
        let mut data = vec![0; 64];
        put32(&mut data, 0, 0x80);
        put32(&mut data, 4, 64);
        data[8] = 1;
        put64(&mut data, 48, 12_345_678);
        let bytes = encoded(vec![resident(0x30, &name), data]);
        let record = parse_record(32, 512, &bytes).unwrap().unwrap();
        assert_eq!(record.names[0].name, text);
        assert_eq!(record.size, Some(12_345_678));
        for length in 0..bytes.len() {
            assert!(parse_record(32, 512, &bytes[..length]).is_err());
        }
        assert!(parse_record(32, 512, &vec![0; 65537]).is_err());
    }
    #[test]
    fn already_applied_fixups_are_accepted_only_for_kernel_record_buffers() {
        let original = encoded(vec![resident(0x80, b"12345")]);
        let mut applied = original.clone();
        for (sector, end) in [(1, 510), (2, 1022)] {
            applied[end..end + 2].copy_from_slice(&original[48 + sector * 2..50 + sector * 2]);
        }
        assert_eq!(
            parse_record(18, 512, &applied).unwrap_err(),
            ParseError::Fixup
        );
        assert_eq!(
            parse_kernel_record(18, 512, &applied)
                .unwrap()
                .unwrap()
                .size,
            Some(5)
        );
        applied[1022..1024].copy_from_slice(&original[1022..1024]);
        assert_eq!(
            parse_kernel_record(18, 512, &applied).unwrap_err(),
            ParseError::Fixup
        );
    }
}

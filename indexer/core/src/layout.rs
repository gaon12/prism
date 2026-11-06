//! Resolves the MFT's metadata extents before any bulk scan. The source is
//! read-only, and can retrieve extension records through the filesystem driver
//! without assuming those records lie in the MFT's first data extent.

use crate::{decoded_record, slice, u16_at, u32_at, u64_at, FileReference, ParseError};
use std::collections::{BTreeMap, HashSet};

const MAX_MFT_BYTES: u64 = 64 * 1024 * 1024 * 1024;
const MAX_LIST_BYTES: u64 = 16 * 1024 * 1024;
const MAX_EXTENTS: usize = 1_000_000;
const MAX_EXTENSION_RECORDS: usize = 65_536;
const BLOCK_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Geometry,
    Corrupt,
    Unsupported,
    Changed,
    /// A surviving base and its attribute-list members changed between reads.
    /// This is not proof that the requested base file was deleted.
    Inconsistent,
    Limit,
    Cancelled,
    TimedOut,
    JournalGap,
    Io(std::io::ErrorKind),
}
pub type Result<T> = std::result::Result<T, Error>;
impl From<ParseError> for Error {
    fn from(_: ParseError) -> Self {
        Self::Corrupt
    }
}
impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        match error.kind() {
            std::io::ErrorKind::Interrupted => Self::Cancelled,
            std::io::ErrorKind::TimedOut => Self::TimedOut,
            kind => Self::Io(kind),
        }
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "NTFS metadata operation failed ({self:?}).")
    }
}
impl std::error::Error for Error {}

#[derive(Clone, Copy, Debug)]
pub struct Geometry {
    pub sector_size: u32,
    pub cluster_size: u32,
    pub record_size: u32,
    pub volume_bytes: u64,
    pub mft_bytes: u64,
    pub serial: u64,
}
impl Geometry {
    pub fn validate(self) -> Result<Self> {
        if !(512..=4096).contains(&self.sector_size)
            || !self.sector_size.is_power_of_two()
            || self.cluster_size < self.sector_size
            || self.cluster_size > 2 * 1024 * 1024
            || !self.cluster_size.is_power_of_two()
            || self.record_size < self.sector_size
            || self.record_size > 65536
            || !self.record_size.is_power_of_two()
            || self.mft_bytes == 0
            || self.mft_bytes > MAX_MFT_BYTES
            || !self.mft_bytes.is_multiple_of(u64::from(self.record_size))
            || self.volume_bytes < self.mft_bytes
            || !self
                .volume_bytes
                .is_multiple_of(u64::from(self.sector_size))
        {
            return Err(Error::Geometry);
        }
        Ok(self)
    }
}

pub struct RecordBuffer {
    pub bytes: Vec<u8>,
    /// True only for completed FSCTL_GET_NTFS_FILE_RECORD output: sector
    /// trailers are already restored and cached edits can leave its USA stale.
    /// Raw volume/test-image bytes must leave this false.
    pub kernel_fixups: bool,
}
impl RecordBuffer {
    fn decode(&self, geometry: Geometry) -> Result<Vec<u8>> {
        if self.bytes.len() != geometry.record_size as usize {
            return Err(Error::Corrupt);
        }
        decoded_record(
            &self.bytes,
            geometry.sector_size as usize,
            self.kernel_fixups,
        )?
        .ok_or(Error::Changed)
    }
}

pub trait Source {
    /// Read exactly the requested physical bytes. Implementations must reject
    /// out-of-volume ranges and use read-only handles. Native I/O must request
    /// cancellation at its deadline and retain storage until completion;
    /// transports separately bound how long a client waits for driver cleanup.
    fn read_exact_at(&mut self, offset: u64, bytes: &mut [u8]) -> Result<()>;
    /// Return this exact MFT number and sequence (if nonzero), never the lower
    /// in-use record that FSCTL_GET_NTFS_FILE_RECORD can return for a hole.
    fn file_record(&mut self, reference: FileReference) -> Result<RecordBuffer>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Extent {
    pub vcn: u64,
    pub clusters: u64,
    pub lcn: u64,
}

#[derive(Clone, Debug)]
struct Part {
    lowest: u64,
    highest: u64,
    size: Option<u64>,
    extents: Vec<Extent>,
}

fn mapping_pairs(attribute: &[u8], geometry: Geometry) -> Result<Part> {
    if attribute.len() < 64 || attribute[8] != 1 || u16_at(attribute, 12)? & 0xc0ff != 0 {
        return Err(Error::Unsupported);
    }
    let lowest = u64_at(attribute, 16)?;
    let highest = u64_at(attribute, 24)?;
    if highest < lowest {
        return Err(Error::Corrupt);
    }
    let mut offset = u16_at(attribute, 32)? as usize;
    if offset < 64 || offset >= attribute.len() {
        return Err(Error::Corrupt);
    }
    let mut vcn = lowest;
    let mut lcn = 0i128;
    let mut extents = vec![];
    loop {
        let header = *attribute.get(offset).ok_or(Error::Corrupt)?;
        offset += 1;
        if header == 0 {
            break;
        }
        let count_len = usize::from(header & 15);
        let offset_len = usize::from(header >> 4);
        if !(1..=8).contains(&count_len) || !(1..=8).contains(&offset_len) {
            return Err(Error::Unsupported);
        }
        let mut count = [0u8; 8];
        count[..count_len].copy_from_slice(slice(attribute, offset, count_len)?);
        offset += count_len;
        let clusters = u64::from_le_bytes(count);
        let delta_bytes = slice(attribute, offset, offset_len)?;
        offset += offset_len;
        let mut delta = if delta_bytes[offset_len - 1] & 0x80 != 0 {
            [255u8; 8]
        } else {
            [0; 8]
        };
        delta[..offset_len].copy_from_slice(delta_bytes);
        lcn = lcn
            .checked_add(i128::from(i64::from_le_bytes(delta)))
            .ok_or(Error::Corrupt)?;
        if clusters == 0 || lcn <= 0 || lcn > i128::from(u64::MAX) {
            return Err(Error::Corrupt);
        }
        let lcn = lcn as u64;
        let physical_end = lcn
            .checked_add(clusters)
            .and_then(|end| end.checked_mul(u64::from(geometry.cluster_size)))
            .ok_or(Error::Corrupt)?;
        if physical_end > geometry.volume_bytes {
            return Err(Error::Corrupt);
        }
        extents.push(Extent { vcn, clusters, lcn });
        vcn = vcn.checked_add(clusters).ok_or(Error::Corrupt)?;
        if extents.len() > MAX_EXTENTS {
            return Err(Error::Limit);
        }
    }
    if vcn != highest.checked_add(1).ok_or(Error::Corrupt)? {
        return Err(Error::Corrupt);
    }
    Ok(Part {
        lowest,
        highest,
        size: (lowest == 0).then(|| u64_at(attribute, 48)).transpose()?,
        extents,
    })
}

fn attributes(record: &[u8]) -> Result<Vec<&[u8]>> {
    let mut result = vec![];
    let mut offset = u16_at(record, 20)? as usize;
    loop {
        if u32_at(record, offset)? == u32::MAX {
            break;
        }
        let length = u32_at(record, offset + 4)? as usize;
        if length < 24 || !length.is_multiple_of(8) {
            return Err(Error::Corrupt);
        }
        let attribute = slice(record, offset, length)?;
        if attribute[8] > 1 {
            return Err(Error::Corrupt);
        }
        result.push(attribute);
        offset = offset.checked_add(length).ok_or(Error::Corrupt)?;
    }
    Ok(result)
}

fn resident(attribute: &[u8]) -> Result<&[u8]> {
    if attribute[8] != 0 {
        return Err(Error::Unsupported);
    }
    let offset = u16_at(attribute, 20)? as usize;
    if offset < 24 {
        return Err(Error::Corrupt);
    }
    Ok(slice(attribute, offset, u32_at(attribute, 16)? as usize)?)
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct ListEntry {
    kind: u32,
    reference: FileReference,
    lowest: u64,
    instance: u16,
}

fn list_entries(bytes: &[u8]) -> Result<Vec<ListEntry>> {
    let mut entries = vec![];
    let mut offset = 0;
    while offset < bytes.len() {
        // NTFS can pad the final initialized sector with zeroes.
        if bytes[offset..].iter().all(|byte| *byte == 0) {
            break;
        }
        let kind = u32_at(bytes, offset)?;
        if kind == u32::MAX {
            break;
        }
        let length = u16_at(bytes, offset + 4)? as usize;
        if length < 26 || !length.is_multiple_of(8) {
            return Err(Error::Corrupt);
        }
        let entry = slice(bytes, offset, length)?;
        if entry[6] == 0 {
            entries.push(ListEntry {
                kind,
                reference: FileReference(u64_at(entry, 16)?),
                lowest: u64_at(entry, 8)?,
                instance: u16_at(entry, 24)?,
            });
        }
        if entries.len() > MAX_EXTENSION_RECORDS {
            return Err(Error::Limit);
        }
        offset = offset.checked_add(length).ok_or(Error::Corrupt)?;
    }
    Ok(entries)
}

fn attribute_list(
    source: &mut impl Source,
    geometry: Geometry,
    record: &[u8],
    check: &mut impl FnMut() -> Result<()>,
) -> Result<Vec<ListEntry>> {
    let mut entries = vec![];
    for attribute in attributes(record)? {
        if attribute[9] != 0 || u32_at(attribute, 0)? != 0x20 {
            continue;
        }
        if attribute[8] == 0 {
            entries.extend(list_entries(resident(attribute)?)?);
        } else {
            let part = mapping_pairs(attribute, geometry)?;
            if part.lowest != 0 {
                return Err(Error::Unsupported);
            }
            let size = part.size.ok_or(Error::Corrupt)?;
            if size > MAX_LIST_BYTES {
                return Err(Error::Limit);
            }
            let extents = checked_extents(BTreeMap::from([(0, part)]), size, geometry)?;
            let mut data = vec![0; size as usize];
            check()?;
            read_mapped(source, geometry, &extents, 0, &mut data)?;
            entries.extend(list_entries(&data)?);
        }
        if entries.len() > MAX_EXTENSION_RECORDS {
            return Err(Error::Limit);
        }
    }
    Ok(entries)
}

/// Fetch the complete current metadata family, including names or the initial
/// unnamed data-size attribute held in extension records. Only $ATTRIBUTE_LIST
/// metadata is followed; user $DATA stream contents are never read.
pub fn metadata_family(
    source: &mut impl Source,
    geometry: Geometry,
    reference: FileReference,
    mut check: impl FnMut() -> Result<()>,
) -> Result<Vec<crate::MetadataRecord>> {
    check()?;
    let mut base = source.file_record(reference)?.decode(geometry)?;
    if reference.sequence() != 0 && u16_at(&base, 16)? != reference.sequence() {
        return Err(Error::Changed);
    }
    let parent = FileReference(u64_at(&base, 32)?);
    let number = if parent.0 != 0 {
        base = source.file_record(parent)?.decode(geometry)?;
        if u16_at(&base, 16)? != parent.sequence() || u64_at(&base, 32)? != 0 {
            return Err(Error::Changed);
        }
        parent.number()
    } else {
        reference.number()
    };
    let base_reference = FileReference(number | (u64::from(u16_at(&base, 16)?) << 48));
    let mut loaded: BTreeMap<u64, Vec<u8>> = BTreeMap::new();
    for entry in attribute_list(source, geometry, &base, &mut check)? {
        if !matches!(entry.kind, 0x10 | 0x30 | 0xc0 | 0x80)
            || (entry.kind == 0x80 && entry.lowest != 0)
        {
            continue;
        }
        check()?;
        let record = if entry.reference == base_reference {
            &base
        } else {
            if let std::collections::btree_map::Entry::Vacant(slot) =
                loaded.entry(entry.reference.0)
            {
                let record = source
                    .file_record(entry.reference)
                    .and_then(|record| record.decode(geometry))
                    .map_err(|error| {
                        if error == Error::Changed {
                            Error::Inconsistent
                        } else {
                            error
                        }
                    })?;
                if u16_at(&record, 16)? != entry.reference.sequence()
                    || u64_at(&record, 32)? != base_reference.0
                {
                    return Err(Error::Inconsistent);
                }
                slot.insert(record);
            }
            &loaded[&entry.reference.0]
        };
        if !attributes(record)?.iter().any(|attribute| {
            u32_at(attribute, 0) == Ok(entry.kind)
                && attribute[9] == 0
                && u16_at(attribute, 14) == Ok(entry.instance)
                && (attribute[8] == 0 || u64_at(attribute, 16) == Ok(entry.lowest))
        }) {
            return Err(Error::Inconsistent);
        }
    }
    let mut family = vec![crate::parse_decoded(number, &base)?];
    for (reference, record) in loaded {
        family.push(crate::parse_decoded(
            FileReference(reference).number(),
            &record,
        )?);
    }
    Ok(family)
}

fn checked_extents(
    parts: BTreeMap<u64, Part>,
    length: u64,
    geometry: Geometry,
) -> Result<Vec<Extent>> {
    if length == 0 {
        return Err(Error::Corrupt);
    }
    let mut extents = vec![];
    let mut next = 0u64;
    for (lowest, part) in parts {
        if lowest != next {
            return Err(Error::Corrupt);
        }
        next = part.highest.checked_add(1).ok_or(Error::Corrupt)?;
        extents.extend(part.extents);
        if extents.len() > MAX_EXTENTS {
            return Err(Error::Limit);
        }
    }
    if next
        .checked_mul(u64::from(geometry.cluster_size))
        .is_none_or(|bytes| bytes < length)
    {
        return Err(Error::Corrupt);
    }
    // A metadata stream cannot map the same physical clusters twice.
    let mut physical: Vec<_> = extents
        .iter()
        .map(|extent| (extent.lcn, extent.clusters))
        .collect();
    physical.sort_unstable();
    for pair in physical.windows(2) {
        if pair[0]
            .0
            .checked_add(pair[0].1)
            .is_none_or(|end| end > pair[1].0)
        {
            return Err(Error::Corrupt);
        }
    }
    Ok(extents)
}

fn read_mapped(
    source: &mut impl Source,
    geometry: Geometry,
    extents: &[Extent],
    mut offset: u64,
    mut output: &mut [u8],
) -> Result<()> {
    let cluster = u64::from(geometry.cluster_size);
    while !output.is_empty() {
        let index =
            extents.partition_point(|extent| extent.vcn + extent.clusters <= offset / cluster);
        let extent = extents.get(index).ok_or(Error::Corrupt)?;
        let start = extent.vcn.checked_mul(cluster).ok_or(Error::Corrupt)?;
        let within = offset.checked_sub(start).ok_or(Error::Corrupt)?;
        let remaining = extent
            .clusters
            .checked_mul(cluster)
            .and_then(|size| size.checked_sub(within))
            .ok_or(Error::Corrupt)?;
        let length = remaining.min(output.len() as u64) as usize;
        if length == 0 {
            return Err(Error::Corrupt);
        }
        let physical = extent
            .lcn
            .checked_mul(cluster)
            .and_then(|start| start.checked_add(within))
            .ok_or(Error::Corrupt)?;
        source.read_exact_at(physical, &mut output[..length])?;
        output = &mut output[length..];
        offset = offset.checked_add(length as u64).ok_or(Error::Corrupt)?;
    }
    Ok(())
}

pub struct MftLayout {
    pub geometry: Geometry,
    pub extents: Vec<Extent>,
    pub base_reference: FileReference,
}
impl MftLayout {
    pub fn load(
        source: &mut impl Source,
        geometry: Geometry,
        mut check: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        let geometry = geometry.validate()?;
        check()?;
        let base = source.file_record(FileReference(0))?.decode(geometry)?;
        if u64_at(&base, 32)? != 0 {
            return Err(Error::Changed);
        }
        let base_reference = FileReference(u64::from(u16_at(&base, 16)?) << 48);
        let base_attributes = attributes(&base)?;
        let mut parts = BTreeMap::new();
        let lists = attribute_list(source, geometry, &base, &mut check)?;
        for attribute in &base_attributes {
            if attribute[9] != 0 {
                continue;
            }
            if u32_at(attribute, 0)? == 0x80 {
                let part = mapping_pairs(attribute, geometry)?;
                if parts.insert(part.lowest, part).is_some() {
                    return Err(Error::Corrupt);
                }
            }
        }
        let size = parts
            .get(&0)
            .and_then(|part| part.size)
            .ok_or(Error::Corrupt)?;
        if size < geometry.mft_bytes || size > MAX_MFT_BYTES {
            return Err(Error::Changed);
        }
        let mut loaded = BTreeMap::new();
        let mut seen = HashSet::new();
        for entry in lists {
            if entry.kind != 0x80 || !seen.insert(entry) {
                continue;
            }
            check()?;
            let record = if entry.reference == base_reference {
                &base
            } else {
                if entry.reference.number() == 0 {
                    return Err(Error::Changed);
                }
                if let std::collections::btree_map::Entry::Vacant(slot) =
                    loaded.entry(entry.reference.0)
                {
                    let record = source.file_record(entry.reference)?.decode(geometry)?;
                    if u16_at(&record, 16)? != entry.reference.sequence()
                        || u64_at(&record, 32)? != base_reference.0
                    {
                        return Err(Error::Changed);
                    }
                    slot.insert(record);
                }
                &loaded[&entry.reference.0]
            };
            let attribute = attributes(record)?
                .into_iter()
                .find(|attribute| {
                    u32_at(attribute, 0) == Ok(0x80)
                        && attribute[9] == 0
                        && u16_at(attribute, 14) == Ok(entry.instance)
                        && u64_at(attribute, 16) == Ok(entry.lowest)
                })
                .ok_or(Error::Changed)?;
            let part = mapping_pairs(attribute, geometry)?;
            if let Some(previous) = parts.get(&part.lowest) {
                if previous.highest != part.highest || previous.extents != part.extents {
                    return Err(Error::Corrupt);
                }
            } else {
                parts.insert(part.lowest, part);
            }
        }
        let extents = checked_extents(parts, geometry.mft_bytes, geometry)?;
        Ok(Self {
            geometry,
            extents,
            base_reference,
        })
    }
    /// Bulk reads follow logical MFT order even across fragmented physical
    /// extents. Progress is based on records read, not a guessed directory count.
    pub fn scan(
        &self,
        source: &mut impl Source,
        mut visit: impl FnMut(u64, &[u8]) -> Result<()>,
        mut check: impl FnMut(u64) -> Result<()>,
    ) -> Result<()> {
        let mut block = vec![0; BLOCK_BYTES];
        let mut offset = 0u64;
        while offset < self.geometry.mft_bytes {
            check(offset / u64::from(self.geometry.record_size))?;
            let length = (self.geometry.mft_bytes - offset).min(BLOCK_BYTES as u64) as usize;
            read_mapped(
                source,
                self.geometry,
                &self.extents,
                offset,
                &mut block[..length],
            )?;
            for (index, record) in block[..length]
                .chunks_exact(self.geometry.record_size as usize)
                .enumerate()
            {
                if index % 256 == 0 {
                    check(offset / u64::from(self.geometry.record_size) + index as u64)?;
                }
                visit(
                    offset / u64::from(self.geometry.record_size) + index as u64,
                    record,
                )?;
            }
            offset += length as u64;
        }
        check(offset / u64::from(self.geometry.record_size))?;
        Ok(())
    }

    /// Transfer ownership of a bounded logical batch to the parser, taking
    /// another reusable buffer back. This avoids copying the complete MFT a
    /// second time merely to send it across a thread boundary. Physical I/O
    /// and all checks stay here, on the source's original calling thread.
    pub(crate) fn scan_owned_batches(
        &self,
        source: &mut impl Source,
        block_bytes: usize,
        mut visit: impl FnMut(u64, Vec<u8>) -> Result<Vec<u8>>,
        mut check: impl FnMut(u64) -> Result<()>,
    ) -> Result<()> {
        let record_bytes = self.geometry.record_size as usize;
        if !(record_bytes..=BLOCK_BYTES).contains(&block_bytes)
            || !block_bytes.is_multiple_of(record_bytes)
        {
            return Err(Error::Limit);
        }
        let mut block = Vec::with_capacity(block_bytes);
        let mut offset = 0u64;
        while offset < self.geometry.mft_bytes {
            let first = offset / record_bytes as u64;
            check(first)?;
            let length = (self.geometry.mft_bytes - offset).min(block_bytes as u64) as usize;
            block.resize(length, 0);
            read_mapped(source, self.geometry, &self.extents, offset, &mut block)?;
            // Progress counts records read, not yet decoded. A parser may be
            // finishing older batches; these checkpoints still poll the real
            // coordinator's cancellation/deadline callback every256 records.
            for index in (0..length / record_bytes).step_by(256) {
                check(first + index as u64)?;
            }
            block = visit(first, block)?;
            offset += length as u64;
        }
        check(offset / record_bytes as u64)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn put16(bytes: &mut [u8], offset: usize, value: u16) {
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }
    fn put32(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    fn put64(bytes: &mut [u8], offset: usize, value: u64) {
        bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }
    fn geometry() -> Geometry {
        Geometry {
            sector_size: 512,
            cluster_size: 512,
            record_size: 1024,
            volume_bytes: 1024 * 1024,
            mft_bytes: 65536,
            serial: 7,
        }
    }
    fn part(lowest: u64, highest: u64, instance: u16, pairs: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0; (64 + pairs.len()).next_multiple_of(8)];
        let size = bytes.len();
        put32(&mut bytes, 0, 0x80);
        put32(&mut bytes, 4, size as u32);
        bytes[8] = 1;
        put16(&mut bytes, 14, instance);
        put64(&mut bytes, 16, lowest);
        put64(&mut bytes, 24, highest);
        put16(&mut bytes, 32, 64);
        put64(&mut bytes, 48, 65536);
        bytes[64..64 + pairs.len()].copy_from_slice(pairs);
        bytes
    }
    fn record(sequence: u16, base: u64, attributes: Vec<Vec<u8>>) -> Vec<u8> {
        let mut bytes = vec![0; 1024];
        bytes[..4].copy_from_slice(b"FILE");
        put16(&mut bytes, 4, 48);
        put16(&mut bytes, 6, 3);
        put16(&mut bytes, 16, sequence);
        put16(&mut bytes, 20, 56);
        put16(&mut bytes, 22, 1);
        put32(&mut bytes, 28, 1024);
        put64(&mut bytes, 32, base);
        let mut offset = 56;
        for attribute in attributes {
            bytes[offset..offset + attribute.len()].copy_from_slice(&attribute);
            offset += attribute.len();
        }
        put32(&mut bytes, offset, u32::MAX);
        put32(&mut bytes, 24, (offset + 4) as u32);
        put16(&mut bytes, 48, 0xbeef);
        for (n, end) in [(1, 510), (2, 1022)] {
            let old = u16_at(&bytes, end).unwrap();
            put16(&mut bytes, 48 + n * 2, old);
            put16(&mut bytes, end, 0xbeef);
        }
        bytes
    }
    fn list(base: u64, extension: u64, lowest: u64) -> Vec<u8> {
        let mut attribute = vec![0; 88];
        put32(&mut attribute, 0, 0x20);
        put32(&mut attribute, 4, 88);
        put32(&mut attribute, 16, 64);
        put16(&mut attribute, 20, 24);
        for (offset, reference, vcn, instance) in [(24, base, 0, 1), (56, extension, lowest, 7)] {
            put32(&mut attribute, offset, 0x80);
            put16(&mut attribute, offset + 4, 32);
            put64(&mut attribute, offset + 8, vcn);
            put64(&mut attribute, offset + 16, reference);
            put16(&mut attribute, offset + 24, instance);
        }
        attribute
    }
    struct Fragmented {
        bytes: Vec<u8>,
        records: BTreeMap<u64, Vec<u8>>,
        reads: Vec<(u64, usize)>,
    }
    impl Source for Fragmented {
        fn read_exact_at(&mut self, offset: u64, output: &mut [u8]) -> Result<()> {
            let start = offset as usize;
            let end = start.checked_add(output.len()).ok_or(Error::Corrupt)?;
            output.copy_from_slice(self.bytes.get(start..end).ok_or(Error::Corrupt)?);
            self.reads.push((offset, output.len()));
            Ok(())
        }
        fn file_record(&mut self, reference: FileReference) -> Result<RecordBuffer> {
            Ok(RecordBuffer {
                bytes: self
                    .records
                    .get(&reference.number())
                    .ok_or(Error::Changed)?
                    .clone(),
                kernel_fixups: false,
            })
        }
    }
    fn fragmented() -> Fragmented {
        let base = 2u64 << 48;
        let extension = (3u64 << 48) | 42;
        let zero = record(
            2,
            0,
            vec![
                part(0, 63, 1, &[0x11, 64, 100, 0]),
                list(base, extension, 64),
            ],
        );
        let extra = record(3, base, vec![part(64, 127, 7, &[0x21, 64, 44, 1, 0])]);
        let mut bytes = vec![0; 1024 * 1024];
        for number in 0..64 {
            let offset = if number < 32 {
                100 * 512 + number * 1024
            } else {
                300 * 512 + (number - 32) * 1024
            };
            bytes[offset..offset + 1024].fill(number as u8);
        }
        Fragmented {
            bytes,
            records: BTreeMap::from([(0, zero), (42, extra)]),
            reads: vec![],
        }
    }
    #[test]
    fn bootstrap_uses_driver_extension_records_and_bulk_reads_fragmented_extents() {
        let mut source = fragmented();
        let layout = MftLayout::load(&mut source, geometry(), || Ok(())).unwrap();
        assert_eq!(
            layout.extents,
            [
                Extent {
                    vcn: 0,
                    clusters: 64,
                    lcn: 100
                },
                Extent {
                    vcn: 64,
                    clusters: 64,
                    lcn: 300
                }
            ]
        );
        let mut seen = vec![];
        layout
            .scan(
                &mut source,
                |number, bytes| {
                    assert!(bytes.iter().all(|byte| *byte == number as u8));
                    seen.push(number);
                    Ok(())
                },
                |_| Ok(()),
            )
            .unwrap();
        assert_eq!(seen, (0..64).collect::<Vec<_>>());
        assert_eq!(source.reads, [(100 * 512, 32768), (300 * 512, 32768)]);
    }
    #[test]
    fn owned_batches_preserve_fragmented_offsets_and_reuse_the_same_buffer() {
        let mut source = fragmented();
        let layout = MftLayout::load(&mut source, geometry(), || Ok(())).unwrap();
        let mut seen = Vec::new();
        let mut buffers = HashSet::new();
        layout
            .scan_owned_batches(
                &mut source,
                8192,
                |first, bytes| {
                    assert_eq!(bytes.len(), 8192);
                    buffers.insert(bytes.as_ptr() as usize);
                    for (index, record) in bytes.as_chunks::<1024>().0.iter().enumerate() {
                        let number = first + index as u64;
                        assert!(record.iter().all(|byte| *byte == number as u8));
                        seen.push(number);
                    }
                    Ok(bytes)
                },
                |_| Ok(()),
            )
            .unwrap();
        assert_eq!(seen, (0..64).collect::<Vec<_>>());
        assert_eq!(buffers.len(), 1);
        assert_eq!(source.reads.len(), 8);
        assert_eq!(
            source.reads.iter().map(|(_, count)| count).sum::<usize>(),
            65536
        );
        assert!(source.reads.iter().all(|(_, count)| *count <= 8192));
    }
    #[test]
    fn owned_batches_reject_invalid_sizes_and_stop_on_cancel_or_failed_reads() {
        use std::cell::Cell;
        let mut source = fragmented();
        let layout = MftLayout::load(&mut source, geometry(), || Ok(())).unwrap();
        for size in [0, 1023, BLOCK_BYTES + 1024] {
            assert_eq!(
                layout.scan_owned_batches(
                    &mut source,
                    size,
                    |_, _| panic!("invalid batch must not be delivered"),
                    |_| Ok(())
                ),
                Err(Error::Limit)
            );
        }
        assert!(source.reads.is_empty());
        let cancelled = Cell::new(false);
        let mut delivered = 0;
        assert_eq!(
            layout.scan_owned_batches(
                &mut source,
                8192,
                |_, bytes| {
                    delivered += 1;
                    cancelled.set(true);
                    Ok(bytes)
                },
                |_| if cancelled.get() {
                    Err(Error::Cancelled)
                } else {
                    Ok(())
                }
            ),
            Err(Error::Cancelled)
        );
        assert_eq!(delivered, 1);
        assert_eq!(source.reads.len(), 1);
        let mut source = fragmented();
        source.bytes.truncate(100 * 512 + 4000);
        assert_eq!(
            layout.scan_owned_batches(
                &mut source,
                8192,
                |_, _| panic!("a failed read cannot deliver a batch"),
                |_| Ok(())
            ),
            Err(Error::Corrupt)
        );
    }
    #[test]
    fn reject_missing_reused_gapped_and_overlapping_extensions() {
        let mut source = fragmented();
        source.records.remove(&42);
        assert!(matches!(
            MftLayout::load(&mut source, geometry(), || Ok(())),
            Err(Error::Changed)
        ));
        let mut source = fragmented();
        put16(source.records.get_mut(&42).unwrap(), 16, 4);
        assert!(matches!(
            MftLayout::load(&mut source, geometry(), || Ok(())),
            Err(Error::Changed)
        ));
        let mut source = fragmented();
        source.records.insert(
            42,
            record(3, 2u64 << 48, vec![part(64, 127, 7, &[0x11, 64, 110, 0])]),
        );
        assert!(matches!(
            MftLayout::load(&mut source, geometry(), || Ok(())),
            Err(Error::Corrupt)
        ));
        let mut source = fragmented();
        source.records.insert(
            0,
            record(
                2,
                0,
                vec![
                    part(0, 63, 1, &[0x11, 64, 100, 0]),
                    list(2u64 << 48, (3u64 << 48) | 42, 65),
                ],
            ),
        );
        source.records.insert(
            42,
            record(3, 2u64 << 48, vec![part(65, 128, 7, &[0x21, 64, 44, 1, 0])]),
        );
        assert!(matches!(
            MftLayout::load(&mut source, geometry(), || Ok(())),
            Err(Error::Corrupt)
        ));
    }
    #[test]
    fn signed_mapping_deltas_sparse_rejection_and_cancellation() {
        let part =
            mapping_pairs(&part(0, 4, 1, &[0x11, 2, 100, 0x11, 3, 246, 0]), geometry()).unwrap();
        assert_eq!(part.extents[1].lcn, 90);
        assert!(matches!(
            mapping_pairs(&super::tests::part(0, 0, 1, &[0x01, 1, 0]), geometry()),
            Err(Error::Unsupported)
        ));
        let mut source = fragmented();
        let layout = MftLayout::load(&mut source, geometry(), || Ok(())).unwrap();
        let mut seen = 0;
        assert_eq!(
            layout.scan(
                &mut source,
                |_, _| {
                    seen += 1;
                    if seen == 3 {
                        Err(Error::Cancelled)
                    } else {
                        Ok(())
                    }
                },
                |_| Ok(())
            ),
            Err(Error::Cancelled)
        );
        assert_eq!(seen, 3);
        let mut source = fragmented();
        assert!(matches!(
            MftLayout::load(&mut source, geometry(), || Err(Error::Cancelled)),
            Err(Error::Cancelled)
        ));
        assert!(source.reads.is_empty());
    }
    #[test]
    fn metadata_replay_replaces_extended_names_without_reading_user_data() {
        let base = (2u64 << 48) | 18;
        let extension = (3u64 << 48) | 42;
        let mut entries = list(base, extension, 0);
        put32(&mut entries, 56, 0x30);
        let mut value = vec![0; 66 + 12];
        put64(&mut value, 0, (1u64 << 48) | 5);
        value[64] = 6;
        value[65] = 1;
        for (index, unit) in "new.md".encode_utf16().enumerate() {
            put16(&mut value, 66 + index * 2, unit);
        }
        let mut name = vec![0; (24 + value.len()).next_multiple_of(8)];
        let length = name.len();
        put32(&mut name, 0, 0x30);
        put32(&mut name, 4, length as u32);
        put16(&mut name, 14, 7);
        put32(&mut name, 16, value.len() as u32);
        put16(&mut name, 20, 24);
        name[24..24 + value.len()].copy_from_slice(&value);
        let mut source = fragmented();
        source.records = BTreeMap::from([
            (
                18,
                record(2, 0, vec![part(0, 63, 1, &[0x11, 64, 100, 0]), entries]),
            ),
            (42, record(3, base, vec![name])),
        ]);
        let family =
            metadata_family(&mut source, geometry(), FileReference(base), || Ok(())).unwrap();
        assert_eq!(family.len(), 2);
        assert!(
            source.reads.is_empty(),
            "unnamed data extents must not be followed for file metadata"
        );
        let mut builder = crate::CatalogBuilder::default();
        builder
            .insert(crate::MetadataRecord {
                reference: FileReference((1u64 << 48) | 5),
                base: None,
                is_dir: true,
                names: vec![],
                size: Some(0),
                modified_ms: None,
                attributes: 0,
                reparse_tag: None,
            })
            .unwrap();
        builder.insert(family[0].clone()).unwrap();
        let mut stale = family[1].clone();
        stale.reference = FileReference((1u64 << 48) | 43);
        stale.names[0].name = "old.md".into();
        builder.insert(stale).unwrap();
        builder.replace_family(family).unwrap();
        builder.remove(FileReference((1u64 << 48) | 18));
        let mut files = vec![];
        builder
            .finish()
            .unwrap()
            .visit(
                prism_ntfs_protocol::DriveLetter::new(b'C').unwrap(),
                |file| {
                    files.push(file);
                    Ok::<_, ()>(())
                },
            )
            .unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[1].path, "C:\\new.md");
        assert_eq!(files[1].size, 65536);
        // An extension can disappear between the base and member reads. The
        // requested BASE is still present: callers must abort/rebuild instead
        // of interpreting this family race as deletion of the base file.
        source.records.remove(&42);
        assert!(matches!(
            metadata_family(&mut source, geometry(), FileReference(base), || Ok(())),
            Err(Error::Inconsistent)
        ));
        // A request for the now-deleted extension itself is a stale reference.
        assert!(matches!(
            metadata_family(&mut source, geometry(), FileReference(extension), || Ok(())),
            Err(Error::Changed)
        ));
    }
}

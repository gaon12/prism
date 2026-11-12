//! Versioned, metadata-only snapshots. Digest validation precedes catalog
//! allocation; callers must separately protect cache files and bind the open
//! source volume to the expected physical kernel device. No payload bytes or
//! derived absolute paths are persisted.
use super::{Geometry, Snapshot};
use crate::{journal::Checkpoint, Catalog, FileName, FileReference, MetadataRecord, Records};
use prism_ntfs_protocol::valid_device_name;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    io::{self, Read, Seek, SeekFrom, Write},
};

const MAGIC: &[u8; 8] = b"PRSMNC01";
const HEADER: u64 = 48;
pub const MAX_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_RECORDS: usize = 10_000_000;
const MAX_RECORD_BYTES: usize = 5 * 1024 * 1024;
fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Invalid NTFS metadata cache.")
}
fn checked(check: &impl Fn() -> super::layout::Result<()>) -> io::Result<()> {
    check().map_err(io::Error::other)
}
fn number<const N: usize>(read: &mut impl Read) -> io::Result<[u8; N]> {
    let mut b = [0; N];
    read.read_exact(&mut b)?;
    Ok(b)
}
fn u32_value(read: &mut impl Read) -> io::Result<u32> {
    Ok(u32::from_le_bytes(number(read)?))
}
fn u64_value(read: &mut impl Read) -> io::Result<u64> {
    Ok(u64::from_le_bytes(number(read)?))
}
fn flag(read: &mut impl Read) -> io::Result<bool> {
    match number::<1>(read)?[0] {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(invalid()),
    }
}
fn optional(read: &mut impl Read) -> io::Result<Option<u64>> {
    let present = flag(read)?;
    let value = u64_value(read)?;
    if !present && value != 0 {
        return Err(invalid());
    }
    Ok(present.then_some(value))
}
fn put_optional(out: &mut Vec<u8>, value: Option<u64>) {
    out.push(u8::from(value.is_some()));
    out.extend_from_slice(&value.unwrap_or(0).to_le_bytes());
}
fn valid_name(name: &str, namespace: u8) -> bool {
    namespace <= 3
        && !name.is_empty()
        && !matches!(name, "." | "..")
        && !name.contains(['\0', '\\', '/', ':'])
        && name.encode_utf16().count() <= 255
}
fn encode_record(record: &MetadataRecord, out: &mut Vec<u8>) -> io::Result<()> {
    if record.base.is_some() || record.names.len() > 4096 || record.is_dir && record.size != Some(0)
    {
        return Err(invalid());
    }
    out.clear();
    out.extend_from_slice(&record.reference.0.to_le_bytes());
    out.push(u8::from(record.is_dir));
    put_optional(out, record.size);
    put_optional(out, record.modified_ms.map(|n| n as u64));
    out.extend_from_slice(&record.attributes.to_le_bytes());
    out.push(u8::from(record.reparse_tag.is_some()));
    out.extend_from_slice(&record.reparse_tag.unwrap_or(0).to_le_bytes());
    out.extend_from_slice(&(record.names.len() as u16).to_le_bytes());
    for name in &record.names {
        if !valid_name(&name.name, name.namespace) {
            return Err(invalid());
        }
        out.extend_from_slice(&name.parent.0.to_le_bytes());
        out.push(name.namespace);
        out.extend_from_slice(&(name.name.len() as u16).to_le_bytes());
        out.extend_from_slice(name.name.as_bytes());
    }
    if out.len() > MAX_RECORD_BYTES {
        return Err(invalid());
    }
    Ok(())
}
fn decode_record(bytes: &[u8]) -> io::Result<MetadataRecord> {
    let mut read = io::Cursor::new(bytes);
    let reference = FileReference(u64_value(&mut read)?);
    let is_dir = flag(&mut read)?;
    let size = optional(&mut read)?;
    let modified_ms = optional(&mut read)?.map(|n| n as i64);
    let attributes = u32_value(&mut read)?;
    let has_tag = flag(&mut read)?;
    let tag = u32_value(&mut read)?;
    if !has_tag && tag != 0 || is_dir && size != Some(0) {
        return Err(invalid());
    }
    let count = u16::from_le_bytes(number(&mut read)?) as usize;
    if count > 4096 {
        return Err(invalid());
    }
    let mut names = Vec::with_capacity(count.min(16));
    for _ in 0..count {
        let parent = FileReference(u64_value(&mut read)?);
        let namespace = number::<1>(&mut read)?[0];
        let length = u16::from_le_bytes(number(&mut read)?) as usize;
        let start = read.position() as usize;
        if length > 1020 || length > bytes.len().saturating_sub(start) {
            return Err(invalid());
        }
        let name = std::str::from_utf8(&bytes[start..start + length]).map_err(|_| invalid())?;
        if !valid_name(name, namespace) {
            return Err(invalid());
        }
        names.push(FileName {
            parent,
            name: name.to_owned(),
            namespace,
        });
        read.set_position((start + length) as u64);
    }
    if read.position() != bytes.len() as u64 {
        return Err(invalid());
    }
    Ok(MetadataRecord {
        reference,
        base: None,
        is_dir,
        size,
        modified_ms,
        attributes,
        reparse_tag: has_tag.then_some(tag),
        names,
        name_budget: 0,
    })
}
struct Body<'a, W> {
    out: &'a mut W,
    digest: Sha256,
    length: u64,
}
impl<W: Write> Body<'_, W> {
    fn put(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.length = self
            .length
            .checked_add(bytes.len() as u64)
            .filter(|n| *n <= MAX_BYTES)
            .ok_or_else(invalid)?;
        self.out.write_all(bytes)?;
        self.digest.update(bytes);
        Ok(())
    }
}
/// Write only to a new empty staging stream, never over an existing cache.
/// The platform publishes it with a same-directory rename after fsync.
pub fn write(
    snapshot: &Snapshot,
    device: &str,
    out: &mut (impl Write + Seek),
    check: impl Fn() -> super::layout::Result<()>,
) -> io::Result<()> {
    checked(&check)?;
    snapshot.geometry.validate().map_err(io::Error::other)?;
    snapshot.checkpoint.validate().map_err(io::Error::other)?;
    if !valid_device_name(device)
        || snapshot
            .catalog
            .records
            .len()
            .saturating_add(snapshot.catalog.owners.len())
            > MAX_RECORDS
        || snapshot
            .catalog
            .families
            .values()
            .any(|members| members.len() > 65536)
        || out.stream_position()? != 0
        || out.seek(SeekFrom::End(0))? != 0
    {
        return Err(invalid());
    }
    out.write_all(&[0; HEADER as usize])?;
    let (length, digest) = {
        let mut body = Body {
            out,
            digest: Sha256::new(),
            length: 0,
        };
        let g = snapshot.geometry;
        let p = snapshot.checkpoint;
        for n in [g.sector_size, g.cluster_size, g.record_size] {
            body.put(&n.to_le_bytes())?;
        }
        for n in [
            g.volume_bytes,
            g.mft_bytes,
            g.serial,
            p.id,
            p.first as u64,
            p.next as u64,
            p.lowest as u64,
        ] {
            body.put(&n.to_le_bytes())?;
        }
        body.put(&(device.len() as u16).to_le_bytes())?;
        body.put(device.as_bytes())?;
        body.put(&(snapshot.catalog.records.len() as u32).to_le_bytes())?;
        let mut bytes = Vec::new();
        for (index, (_, record)) in snapshot.catalog.records.iter().enumerate() {
            if index % 256 == 0 {
                checked(&check)?;
            }
            encode_record(record, &mut bytes)?;
            body.put(&(bytes.len() as u32).to_le_bytes())?;
            body.put(&bytes)?;
        }
        body.put(&(snapshot.catalog.owners.len() as u32).to_le_bytes())?;
        let mut keys: Vec<_> = snapshot.catalog.owners.keys().copied().collect();
        keys.sort_unstable();
        for (index, key) in keys.into_iter().enumerate() {
            if index % 256 == 0 {
                checked(&check)?;
            }
            body.put(&key.to_le_bytes())?;
            body.put(&snapshot.catalog.owners[&key].0.to_le_bytes())?;
        }
        (body.length, body.digest.finalize())
    };
    checked(&check)?;
    out.seek(SeekFrom::Start(0))?;
    out.write_all(MAGIC)?;
    out.write_all(&length.to_le_bytes())?;
    out.write_all(&digest)?;
    out.seek(SeekFrom::Start(HEADER + length))?;
    out.flush()
}

/// Verify the complete bounded payload hash BEFORE allocating record/name
/// collections. A changed device/geometry, trailing bytes or damaged digest
/// rejects the cache; callers must rebuild rather than publish stale paths.
/// The input must remain immutable across the digest and decoding passes;
/// native callers retain an open file handle denying write/delete sharing.
pub fn read(
    input: &mut (impl Read + Seek),
    expected: Geometry,
    device: &str,
    check: impl Fn() -> super::layout::Result<()>,
) -> io::Result<Snapshot> {
    checked(&check)?;
    expected.validate().map_err(io::Error::other)?;
    if !valid_device_name(device) {
        return Err(invalid());
    }
    input.seek(SeekFrom::Start(0))?;
    if &number::<8>(input)? != MAGIC {
        return Err(invalid());
    }
    let length = u64_value(input)?;
    let digest = number::<32>(input)?;
    if length == 0 || length > MAX_BYTES || input.seek(SeekFrom::End(0))? != HEADER + length {
        return Err(invalid());
    }
    input.seek(SeekFrom::Start(HEADER))?;
    let mut remaining = length;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    while remaining > 0 {
        checked(&check)?;
        let take = remaining.min(buffer.len() as u64) as usize;
        input.read_exact(&mut buffer[..take])?;
        hash.update(&buffer[..take]);
        remaining -= take as u64;
    }
    if hash.finalize().as_slice() != digest {
        return Err(invalid());
    }
    input.seek(SeekFrom::Start(HEADER))?;
    let mut body = input.take(length);
    let geometry = Geometry {
        sector_size: u32_value(&mut body)?,
        cluster_size: u32_value(&mut body)?,
        record_size: u32_value(&mut body)?,
        volume_bytes: u64_value(&mut body)?,
        mft_bytes: u64_value(&mut body)?,
        serial: u64_value(&mut body)?,
    }
    .validate()
    .map_err(io::Error::other)?;
    if geometry.serial != expected.serial
        || geometry.sector_size != expected.sector_size
        || geometry.cluster_size != expected.cluster_size
        || geometry.record_size != expected.record_size
        || geometry.volume_bytes != expected.volume_bytes
    {
        return Err(invalid());
    }
    let checkpoint = Checkpoint {
        id: u64_value(&mut body)?,
        first: u64_value(&mut body)? as i64,
        next: u64_value(&mut body)? as i64,
        lowest: u64_value(&mut body)? as i64,
    }
    .validate()
    .map_err(io::Error::other)?;
    let size = u16::from_le_bytes(number(&mut body)?) as usize;
    if size > 1024 {
        return Err(invalid());
    }
    let mut saved = vec![0; size];
    body.read_exact(&mut saved)?;
    let saved = std::str::from_utf8(&saved).map_err(|_| invalid())?;
    if !valid_device_name(saved) || !saved.eq_ignore_ascii_case(device) {
        return Err(invalid());
    }
    let count = u32_value(&mut body)? as usize;
    if count > MAX_RECORDS || count as u64 > body.limit() / 42 {
        return Err(invalid());
    }
    let mut records = Records::with_capacity(count.min(65536));
    let mut previous = None;
    let mut bytes = Vec::new();
    for index in 0..count {
        if index % 256 == 0 {
            checked(&check)?;
        }
        let size = u32_value(&mut body)? as usize;
        if !(38..=MAX_RECORD_BYTES).contains(&size) || size as u64 > body.limit() {
            return Err(invalid());
        }
        bytes.resize(size, 0);
        body.read_exact(&mut bytes)?;
        let record = decode_record(&bytes)?;
        let key = record.reference.number();
        if previous.is_some_and(|last| last >= key) {
            return Err(invalid());
        }
        previous = Some(key);
        records.insert(key, record);
    }
    let count = u32_value(&mut body)? as usize;
    if count > MAX_RECORDS.saturating_sub(records.len()) || count as u64 > body.limit() / 16 {
        return Err(invalid());
    }
    let mut owners = HashMap::with_capacity(count.min(65536));
    let mut families: HashMap<FileReference, Vec<u64>> = HashMap::new();
    previous = None;
    for index in 0..count {
        if index % 256 == 0 {
            checked(&check)?;
        }
        let key = u64_value(&mut body)?;
        let base = FileReference(u64_value(&mut body)?);
        if key > 0x0000_ffff_ffff_ffff
            || records.contains_key(&key)
            || previous.is_some_and(|last| last >= key)
            || !records
                .get(&base.number())
                .is_some_and(|record| record.reference == base)
        {
            return Err(invalid());
        }
        previous = Some(key);
        owners.insert(key, base);
        let members = families.entry(base).or_default();
        if members.len() >= 65536 {
            return Err(invalid());
        }
        members.push(key);
    }
    checked(&check)?;
    if body.limit() != 0 {
        return Err(invalid());
    }
    Ok(Snapshot {
        catalog: Catalog {
            records,
            directories: HashMap::new(),
            owners,
            families,
        },
        geometry,
        checkpoint,
        records_read: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{layout::Error, CatalogBuilder};
    use std::{cell::Cell, io::Cursor};
    const DEVICE: &str = r"\Device\CacheFixture";
    fn sample() -> Snapshot {
        let geometry = Geometry {
            sector_size: 512,
            cluster_size: 512,
            record_size: 1024,
            volume_bytes: 131072,
            mft_bytes: 65536,
            serial: 7,
        };
        let root = MetadataRecord {
            reference: FileReference((1 << 48) | 5),
            base: None,
            is_dir: true,
            names: vec![],
            size: Some(0),
            modified_ms: None,
            attributes: 0,
            reparse_tag: None,
            name_budget: 0,
        };
        let base = MetadataRecord {
            reference: FileReference((2 << 48) | 18),
            base: None,
            is_dir: false,
            names: vec![FileName {
                parent: root.reference,
                name: "한국어.md".into(),
                namespace: 1,
            }],
            size: Some(42),
            modified_ms: Some(-12345),
            attributes: 0,
            reparse_tag: None,
            name_budget: 0,
        };
        let mut extra = base.clone();
        extra.reference = FileReference((3 << 48) | 42);
        extra.base = Some(base.reference);
        extra.names[0].name = "extra.md".into();
        let mut builder = CatalogBuilder::default();
        for record in [root, base, extra] {
            builder.insert(record).unwrap();
        }
        Snapshot {
            catalog: builder.finish().unwrap(),
            geometry,
            records_read: 64,
            checkpoint: Checkpoint {
                id: 99,
                first: 0,
                next: 128,
                lowest: 0,
            },
        }
    }
    fn encoded(snapshot: &Snapshot) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        write(snapshot, DEVICE, &mut out, || Ok(())).unwrap();
        out.into_inner()
    }
    fn rehash(bytes: &mut [u8]) {
        let digest = Sha256::digest(&bytes[HEADER as usize..]);
        bytes[16..48].copy_from_slice(&digest);
    }
    fn load(bytes: Vec<u8>) -> io::Result<Snapshot> {
        read(
            &mut Cursor::new(bytes),
            sample().geometry,
            DEVICE,
            || Ok(()),
        )
    }
    #[test]
    fn roundtrip_is_deterministic_and_retains_old_extension_ownership() {
        let before = sample();
        let bytes = encoded(&before);
        let mut after = load(bytes.clone()).unwrap();
        assert_eq!(after.checkpoint, before.checkpoint);
        assert_eq!(after.records_read, 0);
        assert_eq!(
            after.catalog.owner(FileReference((9 << 48) | 42)),
            Some(FileReference((2 << 48) | 18))
        );
        assert_eq!(after.catalog.records[&18].modified_ms, Some(-12345));
        assert_eq!(after.catalog.records[&5].modified_ms, None);
        assert_eq!(encoded(&after), bytes);
        let mut paths = vec![];
        after
            .catalog
            .visit(
                prism_ntfs_protocol::DriveLetter::new(b'D').unwrap(),
                |file| {
                    paths.push(file.path);
                    Ok::<_, ()>(())
                },
            )
            .unwrap();
        assert_eq!(paths, ["D:\\", "D:\\한국어.md", "D:\\extra.md"]);
        assert!(
            !bytes.windows(3).any(|window| window == b"D:\\"),
            "cache stores parent references and names, not derived drive paths"
        );
        let mut base = after.catalog.records[&18].clone();
        base.names.retain(|name| name.name != "extra.md");
        after.catalog.replace_family(vec![base]).unwrap();
        assert_eq!(after.catalog.owner(FileReference(42)), None);
    }
    #[test]
    fn reject_damage_truncation_new_versions_trailing_bytes_and_large_claims() {
        let bytes = encoded(&sample());
        for length in [0, 7, HEADER as usize - 1, HEADER as usize, bytes.len() - 1] {
            assert!(load(bytes[..length].to_vec()).is_err());
        }
        let mut bad = bytes.clone();
        bad.push(0);
        assert!(load(bad).is_err());
        let mut bad = bytes.clone();
        bad[7] = b'9';
        assert!(load(bad).is_err());
        let mut bad = bytes.clone();
        bad[8..16].copy_from_slice(&(MAX_BYTES + 1).to_le_bytes());
        assert!(load(bad).is_err());
        let mut bad = bytes;
        let last = bad.len() - 1;
        bad[last] ^= 1;
        assert!(load(bad).is_err());
    }
    #[test]
    fn even_correctly_hashed_invalid_counts_names_and_owners_are_rejected() {
        let bytes = encoded(&sample());
        let count = HEADER as usize + 68 + 2 + DEVICE.len();
        let mut bad = bytes.clone();
        bad[count..count + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        rehash(&mut bad);
        assert!(load(bad).is_err());
        let mut bad = bytes.clone();
        bad[count + 4..count + 8].copy_from_slice(&(MAX_RECORD_BYTES as u32 + 1).to_le_bytes());
        rehash(&mut bad);
        assert!(load(bad).is_err());
        let name = bytes
            .windows(8)
            .position(|value| value == b"extra.md")
            .unwrap();
        let mut bad = bytes.clone();
        bad[name..name + 8].copy_from_slice(b"../aa.md");
        rehash(&mut bad);
        assert!(load(bad).is_err());
        let mut bad = bytes.clone();
        bad[name - 3] = 4;
        rehash(&mut bad);
        assert!(load(bad).is_err());
        let mut bad = bytes;
        let offset = bad.len() - 8;
        bad[offset..].copy_from_slice(&((2u64 << 48) | 999).to_le_bytes());
        rehash(&mut bad);
        assert!(load(bad).is_err());
    }
    #[test]
    fn physical_binding_cancellation_and_nonempty_destination_fail_closed() {
        let sample = sample();
        let bytes = encoded(&sample);
        let mut other = sample.geometry;
        other.serial += 1;
        assert!(read(&mut Cursor::new(bytes.clone()), other, DEVICE, || Ok(())).is_err());
        assert!(read(
            &mut Cursor::new(bytes.clone()),
            sample.geometry,
            r"\Device\ClonedSerial",
            || Ok(())
        )
        .is_err());
        // Growing the MFT does not change the physical volume identity. New
        // records are discovered by replay, rather than invalidating all data.
        let mut grown = sample.geometry;
        grown.mft_bytes *= 2;
        assert!(read(&mut Cursor::new(bytes.clone()), grown, DEVICE, || Ok(())).is_ok());
        let calls = Cell::new(0);
        let result = read(&mut Cursor::new(bytes), sample.geometry, DEVICE, || {
            calls.set(calls.get() + 1);
            if calls.get() > 2 {
                Err(Error::Cancelled)
            } else {
                Ok(())
            }
        });
        assert!(result.is_err());
        let mut existing = Cursor::new(b"existing cache".to_vec());
        assert!(write(&sample, DEVICE, &mut existing, || Ok(())).is_err());
        assert_eq!(existing.into_inner(), b"existing cache");
        let mut empty = Cursor::new(Vec::new());
        assert!(write(&sample, DEVICE, &mut empty, || Err(Error::Cancelled)).is_err());
        assert!(empty.into_inner().is_empty());
    }
}

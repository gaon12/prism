//! A deterministic driver/journal FIXTURE around real core replay and write.
//! No live volume, IPC, protected Store, service or HDD is exercised here.
use super::*;
use prism_ntfs_core::{journal, layout, snapshot};
use std::collections::BTreeMap;

struct Fixture {
    start: Checkpoint,
    stop: Checkpoint,
    records: BTreeMap<u64, Option<Vec<u8>>>,
    events: Vec<FileReference>,
    record_reads: usize,
    journal_reads: usize,
    raw_reads: usize,
}
fn edited_record(
    mft: &[u8],
    source: usize,
    number: u64,
    sequence: u16,
    name: &str,
) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut bytes = mft[source * 1024..(source + 1) * 1024].to_vec();
    bytes[16..18].copy_from_slice(&sequence.to_le_bytes());
    bytes[44..48].copy_from_slice(&(number as u32).to_le_bytes());
    let old = if source == 68 {
        "many_subdirs"
    } else {
        "file-with-12345"
    };
    let utf16 = |text: &str| {
        text.encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>()
    };
    let old = utf16(old);
    let name = utf16(name);
    if name.len() != old.len() {
        return Err(input_error("Fixture names must preserve encoded length").into());
    }
    let offset = bytes
        .windows(old.len())
        .position(|part| part == old)
        .ok_or_else(|| input_error("Missing public fixture name"))?;
    bytes[offset..offset + name.len()].copy_from_slice(&name);
    let parsed = parse_record(number, 512, &bytes)
        .map_err(metadata_error)?
        .ok_or_else(|| input_error("Missing fixture metadata"))?;
    // Retarget every FILE_NAME parent; the selected public records have no
    // attribute list. Parent bytes lie before the first sector trailer.
    let mut attribute = u16::from_le_bytes(bytes[20..22].try_into()?) as usize;
    loop {
        let kind = u32::from_le_bytes(bytes[attribute..attribute + 4].try_into()?);
        if kind == u32::MAX {
            break;
        }
        let length = u32::from_le_bytes(bytes[attribute + 4..attribute + 8].try_into()?) as usize;
        if kind == 0x30 {
            let value = attribute
                + u16::from_le_bytes(bytes[attribute + 20..attribute + 22].try_into()?) as usize;
            if value + 8 >= 510 {
                return Err(input_error("Fixture parent crosses a protected sector").into());
            }
            bytes[value..value + 8]
                .copy_from_slice(&reference(if number == 16 { 5 } else { 16 }).0.to_le_bytes());
        }
        attribute += length;
    }
    if parsed.base.is_some() {
        return Err(input_error("Fixture must contain a base record").into());
    }
    Ok(bytes)
}
impl Fixture {
    fn new(count: usize, start: Checkpoint) -> Result<Self, Box<dyn Error>> {
        if count < 16
            || start
                != (Checkpoint {
                    id: 99,
                    first: 0,
                    next: 128,
                    lowest: 0,
                })
        {
            return Err(input_error(
                "Replay requires an original generated cache with at least 16 metadata records",
            )
            .into());
        }
        let directories = (count / 16).clamp(1, 65536);
        let first = 16 + directories as u64;
        let mut mft = Vec::new();
        GzDecoder::new(&include_bytes!("../../tests/fixtures/testfs1-mft.gz")[..])
            .take(3 * 1024 * 1024)
            .read_to_end(&mut mft)?;
        let mut records = BTreeMap::from([
            (16, Some(edited_record(&mft, 68, 16, 1, "replay-dir00")?)),
            (
                first,
                Some(edited_record(&mft, 65, first, 1, "changed-0000000")?),
            ),
            (first + 1, None),
            (
                first + 2,
                Some(edited_record(&mft, 65, first + 2, 2, "reused--0000000")?),
            ),
        ]);
        let added = count as u64 + 112;
        records.insert(
            added,
            Some(edited_record(&mft, 65, added, 1, "created-0000000")?),
        );
        let events = vec![
            reference(16),
            reference(first),
            reference(first + 1),
            reference(first + 2),
            FileReference((2 << 48) | (first + 2)),
            reference(added),
        ];
        let stop = Checkpoint {
            next: start.next + events.len() as i64 * 64,
            ..start
        };
        Ok(Self {
            start,
            stop,
            records,
            events,
            record_reads: 0,
            journal_reads: 0,
            raw_reads: 0,
        })
    }
}
impl layout::Source for Fixture {
    fn read_exact_at(&mut self, _: u64, _: &mut [u8]) -> layout::Result<()> {
        self.raw_reads += 1;
        Err(layout::Error::Unsupported)
    }
    fn file_record(&mut self, wanted: FileReference) -> layout::Result<layout::RecordBuffer> {
        self.record_reads += 1;
        let bytes = self
            .records
            .get(&wanted.number())
            .ok_or(layout::Error::Corrupt)?
            .as_ref()
            .ok_or(layout::Error::Changed)?;
        let sequence = u16::from_le_bytes(bytes[16..18].try_into().unwrap());
        if wanted.sequence() != 0 && wanted.sequence() != sequence {
            return Err(layout::Error::Changed);
        }
        Ok(layout::RecordBuffer {
            bytes: bytes.clone(),
            kernel_fixups: false,
        })
    }
}
impl journal::Source for Fixture {
    fn checkpoint(&mut self) -> layout::Result<Checkpoint> {
        Ok(self.stop)
    }
    fn read_changes(&mut self, id: u64, from: i64) -> layout::Result<Vec<u8>> {
        if id != self.start.id || from != self.start.next || self.journal_reads != 0 {
            return Err(layout::Error::Corrupt);
        }
        self.journal_reads += 1;
        let mut bytes = self.stop.next.to_le_bytes().to_vec();
        for (index, reference) in self.events.iter().enumerate() {
            let mut record = [0u8; 64];
            record[..4].copy_from_slice(&64u32.to_le_bytes());
            record[4..6].copy_from_slice(&2u16.to_le_bytes());
            record[8..16].copy_from_slice(&reference.0.to_le_bytes());
            record[24..32].copy_from_slice(&(from + index as i64 * 64).to_le_bytes());
            record[56..58].copy_from_slice(&2u16.to_le_bytes());
            record[58..60].copy_from_slice(&60u16.to_le_bytes());
            record[60] = b'x';
            bytes.extend(record);
        }
        Ok(bytes)
    }
}
#[derive(Default)]
struct Observations {
    directory: bool,
    changed: bool,
    reused: bool,
    created: bool,
    stale: bool,
}
impl Observations {
    fn observe(&mut self, row: &FileRecord) {
        self.directory |= row.path == r"C:\replay-dir00" && row.is_dir;
        self.changed |= row.path == r"C:\replay-dir00\changed-0000000" && row.size == 5;
        self.reused |= row.path == r"C:\replay-dir00\reused--0000000" && row.size == 5;
        self.created |= row.path == r"C:\replay-dir00\created-0000000" && row.size == 5;
        self.stale |= row.path.starts_with(r"C:\dir-00000000")
            || [
                "public-file-00000001.md",
                "public-file-00000002.md",
                "자료-日本語-😀-00000000.md",
                "linked-00000000.md",
            ]
            .iter()
            .any(|name| row.path.ends_with(name));
    }
    fn validate(&self) -> Result<(), Box<dyn Error>> {
        if !self.directory || !self.changed || !self.reused || !self.created || self.stale {
            return Err(
                input_error("Replay rows failed independent fixture path/metadata checks").into(),
            );
        }
        Ok(())
    }
}
fn output(path: &Path) -> io::Result<BufWriter<File>> {
    Ok(BufWriter::with_capacity(
        1024 * 1024,
        OpenOptions::new().write(true).create_new(true).open(path)?,
    ))
}
pub(super) fn measure(mode: &str, path: &Path, output_path: &Path) -> Result<(), Box<dyn Error>> {
    start_gate()?;
    let started = Instant::now();
    let mut input = BufReader::with_capacity(1024 * 1024, open_immutable(path)?);
    let mut rows = Rows::new(started);
    let mut observed = Observations::default();
    let mut accept = |row: FileRecord| {
        observed.observe(&row);
        rows.accept(row)
    };
    let opened;
    let refreshed;
    let visited;
    let drive = DriveLetter::new(b'C')?;
    let source = if mode == "replay-full" {
        let initial = codec::read(&mut input, geometry(), DEVICE, || Ok(()))?;
        opened = started.elapsed().as_secs_f64();
        let mut fixture = Fixture::new(initial.catalog.record_count(), initial.checkpoint)?;
        let mut snapshot = snapshot::refresh(initial, &mut fixture, geometry(), || Ok(()), |_| {})?;
        refreshed = started.elapsed().as_secs_f64();
        snapshot.catalog.visit(drive, &mut accept)?;
        visited = started.elapsed().as_secs_f64();
        observed.validate()?;
        let mut out = output(output_path)?;
        codec::write(&snapshot, DEVICE, &mut out, || Ok(()))?;
        out.flush()?;
        out.get_ref().sync_all()?;
        fixture
    } else {
        let initial = codec::streaming::Reader::open(input, geometry(), DEVICE, || Ok(()))?;
        opened = started.elapsed().as_secs_f64();
        let mut fixture = Fixture::new(initial.record_count(), initial.checkpoint())?;
        let mut snapshot = initial.refresh(&mut fixture, geometry(), || Ok(()), |_| {})?;
        refreshed = started.elapsed().as_secs_f64();
        snapshot.visit(drive, || Ok(()), &mut accept)?;
        visited = started.elapsed().as_secs_f64();
        observed.validate()?;
        let mut out = output(output_path)?;
        snapshot.write(DEVICE, &mut out, || Ok(()))?;
        out.flush()?;
        out.get_ref().sync_all()?;
        fixture
    };
    if source.raw_reads != 0 || source.record_reads != 6 || source.journal_reads != 1 {
        return Err(input_error("Replay performed unexpected driver/journal reads").into());
    }
    let total = started.elapsed().as_secs_f64();
    println!("{{\"mode\":\"{mode}\",\"synthetic\":true,\"journalDriverFixture\":true,\"startGateUsed\":true,\"records\":{},\"pathBytes\":{},\"openSeconds\":{opened},\"refreshSeconds\":{},\"firstRowSeconds\":{},\"firstBatchSeconds\":{},\"visitSeconds\":{},\"persistSeconds\":{},\"totalSeconds\":{total},\"recordReads\":{},\"journalReads\":{},\"rawMftReads\":{},\"checkpointNext\":{},\"fixturePathChecks\":true,\"rowMetadataSha256\":\"{:x}\"}}", rows.count, rows.path_bytes, refreshed-opened, optional(rows.first_row), optional(rows.first_batch), visited-refreshed, total-visited, source.record_reads, source.journal_reads, source.raw_reads, source.stop.next, rows.digest.finalize());
    Ok(())
}

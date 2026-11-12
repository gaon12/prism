//! Explicit synthetic file-backed cache measurement, never a live-volume or
//! end-to-end service/app/HDD benchmark. Reads wait for G on stdin before I/O
//! or catalog allocation so a parent can apply real process limits first.
use flate2::read::GzDecoder;
use prism_ntfs_core::{
    journal::Checkpoint,
    layout::Geometry,
    parse_record,
    snapshot::{codec, Snapshot},
    CatalogBuilder, FileName, FileReference,
};
use prism_ntfs_protocol::{DriveLetter, FileRecord, MAX_BATCH_RECORDS};
use sha2::{Digest, Sha256};
use std::{
    error::Error,
    fs::{File, OpenOptions},
    io::{self, BufReader, BufWriter, Read, Write},
    path::Path,
    time::Instant,
};

const DEVICE: &str = r"\Device\PrismSyntheticCache";
#[path = "cache_reader_bench/replay.rs"]
mod replay;
fn geometry() -> Geometry {
    Geometry {
        sector_size: 512,
        cluster_size: 512,
        record_size: 1024,
        volume_bytes: 8 << 30,
        mft_bytes: 4 << 30,
        serial: 73,
    }
}
fn reference(number: u64) -> FileReference {
    FileReference((1 << 48) | number)
}
fn input_error(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
fn metadata_error(error: prism_ntfs_core::ParseError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("{error:?}"))
}
fn generate(path: &Path, count: usize) -> Result<(), Box<dyn Error>> {
    if !(2..=4_000_000).contains(&count) {
        return Err(input_error("Use 2..=4000000 synthetic metadata records").into());
    }
    let started = Instant::now();
    let mut mft = Vec::new();
    GzDecoder::new(&include_bytes!("../tests/fixtures/testfs1-mft.gz")[..])
        .take(3 * 1024 * 1024)
        .read_to_end(&mut mft)?;
    let mut root = parse_record(5, 512, &mft[5 * 1024..6 * 1024])
        .map_err(metadata_error)?
        .ok_or_else(|| input_error("Missing public root"))?;
    root.reference = reference(5);
    root.names.clear();
    root.size = Some(0);
    root.modified_ms = Some(-1);
    let template = (16..mft.len() / 1024)
        .find_map(|number| {
            parse_record(number as u64, 512, &mft[number * 1024..(number + 1) * 1024])
                .ok()
                .flatten()
                .filter(|record| {
                    !record.is_dir
                        && record.base.is_none()
                        && record.size.is_some()
                        && !record.names.is_empty()
                })
        })
        .ok_or_else(|| input_error("Missing public regular-file metadata"))?;
    let directories = (count / 16).clamp(1, 65_536).min(count - 1);
    let mut builder = CatalogBuilder::default();
    builder.insert(root.clone()).map_err(metadata_error)?;
    for index in 0..directories {
        let mut record = root.clone();
        record.reference = reference(16 + index as u64);
        let parent = if index < 16 {
            reference(5)
        } else {
            reference(16 + (index / 16) as u64)
        };
        record.names = vec![FileName {
            parent,
            name: format!("project-{index:05}"),
            namespace: 1,
        }];
        builder.insert(record).map_err(metadata_error)?;
    }
    for index in 0..count - directories - 1 {
        let mut record = template.clone();
        let number = 16 + directories as u64 + index as u64;
        record.reference = reference(number);
        record.size = Some(number * 17);
        record.modified_ms = if index.is_multiple_of(7) {
            None
        } else {
            Some(if index.is_multiple_of(113) {
                i64::MIN
            } else {
                1609459200000 + index as i64
            })
        };
        let parent = reference(16 + (index % directories) as u64);
        record.names = vec![FileName {
            parent,
            name: if index.is_multiple_of(8) {
                format!("자료-日本語-😀-{index:08}.md")
            } else {
                format!("public-file-{index:08}.md")
            },
            namespace: 1,
        }];
        if index.is_multiple_of(97) {
            record.names.push(FileName {
                parent: reference(16 + ((index + 1) % directories) as u64),
                name: format!("linked-{index:08}.md"),
                namespace: 1,
            });
        }
        if index.is_multiple_of(211) {
            record.names.push(FileName {
                parent,
                name: format!("F{index:07}.MD"),
                namespace: 2,
            });
        }
        builder.insert(record).map_err(metadata_error)?;
    }
    let snapshot = Snapshot {
        catalog: builder.finish().map_err(metadata_error)?,
        checkpoint: Checkpoint {
            id: 99,
            first: 0,
            next: 128,
            lowest: 0,
        },
        records_read: count as u64,
        geometry: geometry(),
    };
    let mut output = BufWriter::with_capacity(
        1024 * 1024,
        OpenOptions::new().write(true).create_new(true).open(path)?,
    );
    codec::write(&snapshot, DEVICE, &mut output, || Ok(()))?;
    output.flush()?;
    println!("{{\"mode\":\"generate\",\"synthetic\":true,\"metadataRecords\":{count},\"directoryRecords\":{},\"seconds\":{}}}", directories+1, started.elapsed().as_secs_f64());
    Ok(())
}
fn open_immutable(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1); // FILE_SHARE_READ only: deny write and delete.
    }
    options.open(path)
}
fn start_gate() -> Result<(), Box<dyn Error>> {
    println!("{{\"waitingForStart\":true}}");
    io::stdout().flush()?;
    let mut start_byte = [0];
    if io::stdin().read_exact(&mut start_byte).is_err() || start_byte != *b"G" {
        return Err(input_error("Expected G before cache access").into());
    }
    Ok(())
}
struct Rows {
    started: Instant,
    count: u64,
    path_bytes: u64,
    digest: Sha256,
    first_row: Option<f64>,
    first_batch: Option<f64>,
}
impl Rows {
    fn new(started: Instant) -> Self {
        Self {
            started,
            count: 0,
            path_bytes: 0,
            digest: Sha256::new(),
            first_row: None,
            first_batch: None,
        }
    }
    fn accept(&mut self, row: FileRecord) -> io::Result<()> {
        self.count += 1;
        self.path_bytes += row.path.len() as u64;
        if self.first_row.is_none() {
            self.first_row = Some(self.started.elapsed().as_secs_f64());
        }
        if self.count == MAX_BATCH_RECORDS as u64 {
            self.first_batch = Some(self.started.elapsed().as_secs_f64());
        }
        self.digest.update((row.path.len() as u64).to_le_bytes());
        self.digest.update(row.path.as_bytes());
        self.digest.update([u8::from(row.is_dir)]);
        self.digest.update(row.size.to_le_bytes());
        self.digest.update([u8::from(row.modified_ms.is_some())]);
        self.digest
            .update(row.modified_ms.unwrap_or(0).to_le_bytes());
        Ok(())
    }
}
fn optional(value: Option<f64>) -> String {
    value.map_or_else(|| "null".into(), |value| value.to_string())
}
fn measure(mode: &str, path: &Path) -> Result<(), Box<dyn Error>> {
    start_gate()?;
    let started = Instant::now();
    let input = BufReader::with_capacity(1024 * 1024, open_immutable(path)?);
    let drive = DriveLetter::new(b'C')?;
    let mut rows = Rows::new(started);
    let opened;
    if mode == "full" {
        let mut input = input;
        let mut snapshot = codec::read(&mut input, geometry(), DEVICE, || Ok(()))?;
        opened = started.elapsed().as_secs_f64();
        snapshot.catalog.visit(drive, |row| rows.accept(row))?;
    } else {
        let mut reader = codec::streaming::Reader::open(input, geometry(), DEVICE, || Ok(()))?;
        opened = started.elapsed().as_secs_f64();
        reader.visit(drive, || Ok(()), |row| rows.accept(row))?;
    }
    let total = started.elapsed().as_secs_f64();
    println!("{{\"mode\":\"{mode}\",\"synthetic\":true,\"cacheCheckpointOnly\":true,\"startGateUsed\":true,\"records\":{},\"pathBytes\":{},\"openSeconds\":{opened},\"firstRowSeconds\":{},\"firstBatchSeconds\":{},\"batchRecords\":{MAX_BATCH_RECORDS},\"visitSeconds\":{},\"totalSeconds\":{total},\"rowMetadataSha256\":\"{:x}\"}}", rows.count, rows.path_bytes, optional(rows.first_row), optional(rows.first_batch), total-opened, rows.digest.finalize());
    Ok(())
}
fn run() -> Result<(), Box<dyn Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    match args.as_slice() {
        [mode, path, count] if mode == "generate" => generate(Path::new(path), count.parse()?),
        [mode, path] if matches!(mode.as_str(), "full" | "stream") => measure(mode, Path::new(path)),
        [mode, path, output] if matches!(mode.as_str(), "replay-full" | "replay-stream") => replay::measure(mode, Path::new(path), Path::new(output)),
        _ => Err(input_error("Usage: cache_reader_bench generate <new-cache> <count> | full|stream <synthetic-cache> | replay-full|replay-stream <synthetic-cache> <new-output>; reads require G on stdin").into()),
    }
}
fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(2);
    }
}

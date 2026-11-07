//! Explicit synthetic codec/receiver comparison; no service, disk, or model.
//! Producer work is excluded equally. Both paths decode and consume every row.
use prism_ntfs_protocol::{
    stream::{Event, Receiver},
    DriveLetter, FileRecord, Request, Response,
};
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    io::Cursor,
    time::{Duration, Instant},
};

fn frames(count: usize, drive: DriveLetter) -> Vec<Vec<u8>> {
    let mut frames = Vec::new();
    let mut header = Vec::new();
    Response::Volume {
        serial: 7,
        device: r"\Device\PrismSyntheticCodec".into(),
    }
    .write(&mut header, drive)
    .unwrap();
    frames.push(header);
    for first in (0..count).step_by(2048) {
        let records = (first..(first + 2048).min(count)).map(|number| {
            let label = match number % 8 {
                0 => "자료-한글-日本語", 1 => "가-😀", _ => "public-source",
            };
            FileRecord {
                path: format!(r"C:\PrismSynthetic\workspace-{:05}\projects\documents\{number:08}-{label}.md", number.wrapping_mul(7919) % 10007),
                is_dir: number.is_multiple_of(97),
                size: number as u64 * 17,
                modified_ms: (!number.is_multiple_of(7)).then_some(1609459200000 + number as i64),
            }
        }).collect();
        let mut frame = Vec::new();
        Response::Records(records).write(&mut frame, drive).unwrap();
        frames.push(frame);
    }
    let mut end = Vec::new();
    Response::Finished { revision: 3 }
        .write(&mut end, drive)
        .unwrap();
    frames.push(end);
    frames
}

fn consume(total: &mut (u64, u64), batch: Vec<FileRecord>) -> usize {
    let count = batch.len();
    for row in batch {
        let mut hash = DefaultHasher::new();
        row.path.hash(&mut hash);
        row.is_dir.hash(&mut hash);
        row.size.hash(&mut hash);
        row.modified_ms.hash(&mut hash);
        let value = hash.finish();
        total.0 ^= value;
        total.1 = total.1.wrapping_add(value);
    }
    count
}

fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let count = match args.as_slice() {
        [] => 1_000_000,
        [flag, value] if flag == "--records" => value.parse::<usize>().unwrap_or(0),
        _ => 0,
    };
    if !(1..=2_500_000).contains(&count) {
        eprintln!("Usage: stream_validation [--records 1..2500000]");
        std::process::exit(2);
    }
    let drive = DriveLetter::new(b'C').unwrap();
    let frames = frames(count, drive);
    let mut expected = None;
    for round in 0..3 {
        // Reverse the order on alternate rounds, keeping the same wire bytes.
        let variants = if round % 2 == 0 {
            [false, true]
        } else {
            [true, false]
        };
        for validated in variants {
            let start = Instant::now();
            let mut hashing = Duration::ZERO;
            let mut metadata = (0, 0);
            let mut seen = 0;
            let mut receiver = Receiver::new(
                Request::Snapshot { drive },
                7,
                r"\Device\PrismSyntheticCodec",
            );
            for frame in &frames {
                let mut cursor = Cursor::new(frame.as_slice());
                let event = if validated {
                    receiver.accept_validated(Response::read_validated(&mut cursor, drive).unwrap())
                } else {
                    receiver.accept(Response::read(&mut cursor, drive).unwrap())
                }
                .unwrap();
                if let Some(Event::Records(batch)) = event {
                    let hash_start = Instant::now();
                    seen += consume(&mut metadata, batch);
                    hashing += hash_start.elapsed();
                }
            }
            let elapsed = start.elapsed();
            assert!(receiver.finished());
            assert_eq!(seen, count);
            if let Some(expected) = expected {
                assert_eq!(metadata, expected);
            } else {
                expected = Some(metadata);
            }
            println!(
                "{{\"synthetic\":true,\"round\":{round},\"validatedCodec\":{validated},\"records\":{seen},\"totalMs\":{:.3},\"decodeValidateMs\":{:.3},\"metadataMatches\":true,\"checksumXor\":\"{:016x}\",\"checksumSum\":\"{:016x}\"}}",
                elapsed.as_secs_f64() * 1000.0,
                elapsed.saturating_sub(hashing).as_secs_f64() * 1000.0,
                metadata.0, metadata.1
            );
        }
    }
}

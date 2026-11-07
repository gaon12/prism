//! Explicit unit-only throughput check on a real, private Windows pipe.
//! No installed service, volume, user-file metadata or model is accessed.
use super::{Connection, Server};
use prism_ntfs_protocol::{
    stream::{Event, Receiver},
    Acknowledgement, DriveLetter, FileRecord, Request, Response,
};
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    io::{self, Read, Write},
    sync::{atomic::AtomicBool, mpsc, Arc},
    time::{Duration, Instant},
};

fn frames(count: usize, drive: DriveLetter) -> Vec<Vec<u8>> {
    let mut frames = Vec::new();
    let mut volume = Vec::new();
    Response::Volume {
        serial: 7,
        device: r"\Device\PrismSyntheticFrames".into(),
    }
    .write(&mut volume, drive)
    .unwrap();
    frames.push(volume);
    for first in (0..count).step_by(2048) {
        let rows = (first..(first + 2048).min(count)).map(|number| FileRecord {
            path: format!(r"C:\PrismSynthetic\workspace-{:05}\projects\documents\source-and-notes\{number:08}-{}.md",
                number.wrapping_mul(7919) % 10007,
                if number.is_multiple_of(8) { "자료-가-日本語-😀" } else { "public-source" }),
            is_dir: number.is_multiple_of(97),
            size: number as u64 * 17,
            modified_ms: (!number.is_multiple_of(7)).then_some(1609459200000 + number as i64),
        }).collect();
        let mut frame = Vec::new();
        Response::Records(rows).write(&mut frame, drive).unwrap();
        frames.push(frame);
    }
    let mut end = Vec::new();
    Response::Finished { revision: 3 }
        .write(&mut end, drive)
        .unwrap();
    frames.push(end);
    frames
}

struct Writer<'a> {
    pipe: &'a mut Connection,
    calls: u64,
    bytes: u64,
}
impl Write for Writer<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.calls += 1;
        let count = self.pipe.write(bytes)?;
        self.bytes += count as u64;
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.pipe.flush()
    }
}
struct Reader<'a> {
    pipe: &'a mut Connection,
    calls: u64,
    bytes: u64,
}
impl Read for Reader<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.calls += 1;
        let count = self.pipe.read(bytes)?;
        self.bytes += count as u64;
        Ok(count)
    }
}

#[test]
#[ignore = "explicit synthetic real-private-pipe framing comparison; no installed service or live volume"]
fn native_frame_write_comparison() {
    let count = std::env::var("PRISM_FRAME_BENCH_RECORDS")
        .map_or(1_000_000, |value| value.parse::<usize>().unwrap());
    assert!((1..=2_500_000).contains(&count));
    let drive = DriveLetter::new(b'C').unwrap();
    // Both variants receive these exact bytes. Producer work is excluded;
    // the extra whole-frame allocation/copy is deliberately timed below.
    let frames = frames(count, drive);
    let mut expected = None;
    for round in 0..3 {
        let variants = if round % 2 == 0 { [4, 2, 1] } else { [1, 2, 4] };
        for pieces in variants {
            let name = format!(
                r"\\.\pipe\Prism.indexer.test.frames.{}.{}.{}",
                std::process::id(),
                round,
                pieces
            );
            std::thread::scope(|scope| {
                let (ready, waiting) = mpsc::channel();
                let name = &name;
                let frames = &frames;
                let server = scope.spawn(move || -> io::Result<(u64, u64, Duration)> {
                    let mut server = Server::fixture(name)?;
                    ready.send(()).unwrap();
                    let mut pipe =
                        server.accept(Arc::new(AtomicBool::new(false)), Duration::from_secs(3))?;
                    assert_eq!(Request::read(&mut pipe)?, Request::Snapshot { drive });
                    let start = Instant::now();
                    let (calls, bytes) = {
                        let mut writer = Writer {
                            pipe: &mut pipe,
                            calls: 0,
                            bytes: 0,
                        };
                        for frame in frames {
                            writer.pipe.set_frame_timeout(Duration::from_secs(20));
                            match pieces {
                                4 => {
                                    writer.write_all(&frame[..4])?;
                                    writer.write_all(&frame[4..6])?;
                                    writer.write_all(&frame[6..10])?;
                                    writer.write_all(&frame[10..])?;
                                }
                                2 => {
                                    writer.write_all(&frame[..10])?;
                                    writer.write_all(&frame[10..])?;
                                }
                                1 => {
                                    // Models the proposed extra contiguous
                                    // buffer, including allocation and copy.
                                    let copied = frame.clone();
                                    writer.write_all(&copied)?;
                                }
                                _ => unreachable!(),
                            }
                        }
                        (writer.calls, writer.bytes)
                    };
                    pipe.set_frame_timeout(Duration::from_secs(3));
                    assert_eq!(
                        Acknowledgement::read(&mut pipe)?,
                        Acknowledgement::Complete { revision: 3 }
                    );
                    Ok((calls, bytes, start.elapsed()))
                });
                waiting.recv().unwrap();
                let mut client = Connection::fixture_client(name).unwrap();
                let start = Instant::now();
                Request::Snapshot { drive }.write(&mut client).unwrap();
                let mut receiver = Receiver::new(
                    Request::Snapshot { drive },
                    7,
                    r"\Device\PrismSyntheticFrames",
                );
                let mut seen = 0;
                let mut metadata = (0u64, 0u64);
                let (calls, bytes) = {
                    let mut reader = Reader {
                        pipe: &mut client,
                        calls: 0,
                        bytes: 0,
                    };
                    while !receiver.finished() {
                        reader.pipe.set_frame_timeout(Duration::from_secs(20));
                        let response = Response::read_validated(&mut reader, drive).unwrap();
                        if let Some(Event::Records(rows)) =
                            receiver.accept_validated(response).unwrap()
                        {
                            seen += rows.len();
                            for row in rows {
                                let mut hash = DefaultHasher::new();
                                row.path.hash(&mut hash);
                                row.is_dir.hash(&mut hash);
                                row.size.hash(&mut hash);
                                row.modified_ms.hash(&mut hash);
                                let value = hash.finish();
                                metadata.0 ^= value;
                                metadata.1 = metadata.1.wrapping_add(value);
                            }
                        }
                    }
                    (reader.calls, reader.bytes)
                };
                assert_eq!(seen, count);
                if let Some(expected) = expected {
                    assert_eq!(metadata, expected);
                } else {
                    expected = Some(metadata);
                }
                Acknowledgement::Complete { revision: 3 }
                    .write(&mut client)
                    .unwrap();
                let elapsed = start.elapsed();
                let (write_calls, write_bytes, server_elapsed) = server.join().unwrap().unwrap();
                assert_eq!(bytes, write_bytes);
                println!("{{\"synthetic\":true,\"nativePrivatePipe\":true,\"round\":{round},\"pieces\":{pieces},\"records\":{seen},\"wireBytes\":{bytes},\"writeCalls\":{write_calls},\"readCalls\":{calls},\"clientMs\":{:.3},\"serverMs\":{:.3},\"metadataMatches\":true,\"checksumXor\":\"{:016x}\",\"checksumSum\":\"{:016x}\"}}", elapsed.as_secs_f64()*1000.0, server_elapsed.as_secs_f64()*1000.0, metadata.0, metadata.1);
            });
        }
    }
}

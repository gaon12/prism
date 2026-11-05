//! Bounded request dispatch for the service process. The production backend
//! reads real NTFS metadata; test backends are supplied only by unit tests.
use crate::{
    pipe::{Connection, Server},
    volume::Volume,
};
use prism_ntfs_core::{
    layout::{self, Error},
    snapshot,
};
use prism_ntfs_protocol::{
    Acknowledgement, FailureCode, Request, Response, MAX_BATCH_RECORDS, MAX_FRAME_BYTES,
};
use std::{
    io,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::JoinHandle,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const MAX_WORKERS: usize = 2;
pub trait Backend: Send + Sync + 'static {
    fn stream(
        &self,
        request: Request,
        cancel: Arc<AtomicBool>,
        shutdown: Arc<AtomicBool>,
        emit: &mut dyn FnMut(Response) -> io::Result<()>,
    ) -> Result<u64, FailureCode>;
}

pub struct SnapshotBackend;
impl Backend for SnapshotBackend {
    fn stream(
        &self,
        request: Request,
        cancel: Arc<AtomicBool>,
        shutdown: Arc<AtomicBool>,
        emit: &mut dyn FnMut(Response) -> io::Result<()>,
    ) -> Result<u64, FailureCode> {
        let drive = match request {
            Request::Snapshot { drive } => drive,
            Request::Changes { .. } => return Err(FailureCode::JournalGap),
        };
        let begin = Instant::now();
        let check = || {
            if shutdown.load(Ordering::Acquire) {
                cancel.store(true, Ordering::Release);
            }
            if cancel.load(Ordering::Acquire) {
                Err(Error::Cancelled)
            } else if begin.elapsed() > Duration::from_secs(110) {
                cancel.store(true, Ordering::Release);
                Err(Error::TimedOut)
            } else {
                Ok(())
            }
        };
        check().map_err(failure)?;
        let mut volume =
            Volume::open(drive, cancel.clone()).map_err(|error| failure(error.into()))?;
        let geometry = volume.geometry();
        emit(Response::Volume {
            serial: geometry.serial,
        })
        .map_err(|_| FailureCode::Cancelled)?;
        let mut last = Instant::now();
        let mut result = snapshot::collect(&mut volume, geometry, check, |progress| {
            if last.elapsed() >= Duration::from_millis(500) {
                let completed = match progress {
                    snapshot::Progress::Mft { processed, .. } => processed,
                    snapshot::Progress::Replay { processed, .. } => {
                        geometry.mft_bytes / u64::from(geometry.record_size) + processed
                    }
                };
                if emit(Response::Progress { completed }).is_err() {
                    cancel.store(true, Ordering::Release);
                }
                last = Instant::now();
            }
        })
        .map_err(failure)?;
        let mut batch = vec![];
        let mut encoded = 5usize;
        result
            .catalog
            .visit(drive, |record| -> Result<(), FailureCode> {
                check().map_err(failure)?;
                let length = record.path.len() + 22;
                if !batch.is_empty()
                    && (batch.len() == MAX_BATCH_RECORDS
                        || encoded.saturating_add(length) > MAX_FRAME_BYTES)
                {
                    emit(Response::Records(std::mem::take(&mut batch)))
                        .map_err(|_| FailureCode::Cancelled)?;
                    encoded = 5;
                }
                encoded += length;
                batch.push(record);
                Ok(())
            })?;
        if !batch.is_empty() {
            emit(Response::Records(batch)).map_err(|_| FailureCode::Cancelled)?;
        }
        check().map_err(failure)?;
        Ok((SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| FailureCode::Unavailable)?
            .as_nanos() as u64)
            .max(1))
    }
}
fn failure(error: layout::Error) -> FailureCode {
    match error {
        Error::Cancelled => FailureCode::Cancelled,
        Error::JournalGap | Error::Changed => FailureCode::JournalGap,
        Error::Unsupported => FailureCode::UnsupportedVolume,
        Error::Io(io::ErrorKind::PermissionDenied) => FailureCode::AccessDenied,
        Error::Corrupt | Error::Geometry => FailureCode::CorruptVolume,
        _ => FailureCode::Unavailable,
    }
}

fn terminal(
    connection: &mut Connection,
    response: Response,
    ack: Acknowledgement,
    drive: prism_ntfs_protocol::DriveLetter,
) -> io::Result<()> {
    connection.set_frame_timeout(Duration::from_secs(3));
    response.write(connection, drive)?;
    connection.set_frame_timeout(Duration::from_secs(2));
    if Acknowledgement::read(connection)? != ack {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Unexpected filename index acknowledgement.",
        ));
    }
    Ok(())
}
fn fail(
    connection: &mut Connection,
    code: FailureCode,
    drive: prism_ntfs_protocol::DriveLetter,
) -> io::Result<()> {
    terminal(
        connection,
        Response::Failure(code),
        Acknowledgement::Failure(code),
        drive,
    )
}
pub(crate) fn session(mut connection: Connection, backend: &dyn Backend) -> io::Result<()> {
    connection.set_frame_timeout(Duration::from_secs(2));
    let request = match Request::read(&mut connection) {
        Ok(request) => request,
        Err(_) => {
            return fail(
                &mut connection,
                FailureCode::InvalidRequest,
                prism_ntfs_protocol::DriveLetter::new(b'C')?,
            )
        }
    };
    let drive = match request {
        Request::Snapshot { drive } | Request::Changes { drive, .. } => drive,
    };
    let cancel = connection.cancel_flag();
    let shutdown = connection.shutdown_flag();
    let result = backend.stream(request, cancel.clone(), shutdown, &mut |response| {
        connection.set_frame_timeout(Duration::from_secs(5));
        let result = response.write(&mut connection, drive);
        if result.is_err() {
            cancel.store(true, Ordering::Release);
        }
        result
    });
    let result = match result {
        Ok(revision) if revision != 0 => terminal(
            &mut connection,
            Response::Finished { revision },
            Acknowledgement::Complete { revision },
            drive,
        ),
        Ok(_) => fail(&mut connection, FailureCode::Unavailable, drive),
        Err(code) => fail(&mut connection, code, drive),
    };
    cancel.store(true, Ordering::Release);
    result
}

struct Worker {
    thread: JoinHandle<()>,
    cancel: Arc<AtomicBool>,
}
struct Workers(Vec<Worker>);
impl Drop for Workers {
    fn drop(&mut self) {
        for worker in &self.0 {
            worker.cancel.store(true, Ordering::Release);
        }
    }
}
pub fn serve(backend: Arc<dyn Backend>, shutdown: Arc<AtomicBool>) -> io::Result<()> {
    serve_ready(backend, shutdown, || {})
}
pub(crate) fn serve_ready(
    backend: Arc<dyn Backend>,
    shutdown: Arc<AtomicBool>,
    ready: impl FnOnce(),
) -> io::Result<()> {
    let mut server = Server::new()?;
    ready();
    let mut guard = Workers(vec![]);
    let workers = &mut guard.0;
    while !shutdown.load(Ordering::Acquire) {
        let mut index = 0;
        while index < workers.len() {
            if workers[index].thread.is_finished() {
                let worker = workers.swap_remove(index);
                let _ = worker.thread.join();
            } else {
                index += 1;
            }
        }
        let mut connection = match server.accept(shutdown.clone(), Duration::from_millis(200)) {
            Ok(connection) => connection,
            Err(error) if error.kind() == io::ErrorKind::TimedOut => continue,
            Err(error) if shutdown.load(Ordering::Acquire) => {
                let _ = error;
                break;
            }
            Err(error) => return Err(error),
        };
        if workers.len() >= MAX_WORKERS {
            let _ = fail(
                &mut connection,
                FailureCode::Busy,
                prism_ntfs_protocol::DriveLetter::new(b'C')?,
            );
            continue;
        }
        let cancel = connection.cancel_flag();
        let backend = backend.clone();
        workers.push(Worker {
            cancel,
            thread: std::thread::spawn(move || {
                let _ = session(connection, backend.as_ref());
            }),
        });
    }
    for worker in workers.iter() {
        worker.cancel.store(true, Ordering::Release);
    }
    // Native driver cleanup can outlive a cancelled request. The service entry
    // point owns final process shutdown; do not block its stop acknowledgement
    // forever waiting for a defective driver's worker to exit.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use prism_ntfs_protocol::{stream::Event, DriveLetter, FileRecord};
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    struct Fixture;
    impl Backend for Fixture {
        fn stream(
            &self,
            request: Request,
            _: Arc<AtomicBool>,
            _: Arc<AtomicBool>,
            emit: &mut dyn FnMut(Response) -> io::Result<()>,
        ) -> Result<u64, FailureCode> {
            if matches!(request, Request::Changes { .. }) {
                return Err(FailureCode::JournalGap);
            }
            emit(Response::Volume { serial: 7 }).unwrap();
            emit(Response::Progress { completed: 581 }).unwrap();
            emit(Response::Records(vec![FileRecord {
                path: "C:\\fixture.md".into(),
                is_dir: false,
                size: 5,
                modified_ms: Some(42),
            }]))
            .map_err(|_| FailureCode::Cancelled)?;
            Ok(11)
        }
    }
    fn pair() -> (
        String,
        std::sync::mpsc::Receiver<()>,
        JoinHandle<io::Result<()>>,
    ) {
        let name = format!(
            r"\\.\pipe\Prism.host.test.{}.{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let server_name = name.clone();
        let (ready, waiting) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            let mut server = Server::fixture(&server_name)?;
            ready.send(()).unwrap();
            session(
                server.accept(Arc::new(AtomicBool::new(false)), Duration::from_secs(2))?,
                &Fixture,
            )
        });
        (name, waiting, thread)
    }
    #[test]
    fn exchange_delivers_all_frames_then_acknowledges_consumption() {
        let (name, ready, thread) = pair();
        ready.recv().unwrap();
        let mut connection = Connection::fixture_client(&name).unwrap();
        let mut events = vec![];
        let revision = connection
            .exchange(
                Request::Snapshot {
                    drive: DriveLetter::new(b'C').unwrap(),
                },
                7,
                |event| {
                    events.push(event);
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(revision, 11);
        assert!(
            matches!(events.as_slice(),[Event::Progress{completed:581},Event::Records(records)] if records.len()==1&&records[0].path=="C:\\fixture.md")
        );
        thread.join().unwrap().unwrap();
    }
    #[test]
    fn failures_are_acknowledged_without_inventing_a_successful_revision() {
        let (name, ready, thread) = pair();
        ready.recv().unwrap();
        let mut connection = Connection::fixture_client(&name).unwrap();
        let mut count = 0;
        let result = connection.exchange(
            Request::Changes {
                drive: DriveLetter::new(b'C').unwrap(),
                after: 11,
            },
            7,
            |_| {
                count += 1;
                Ok(())
            },
        );
        assert!(result.is_err());
        assert_eq!(count, 0);
        thread.join().unwrap().unwrap();
    }
}

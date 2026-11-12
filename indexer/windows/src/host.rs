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
    cell::{Cell, RefCell},
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

#[derive(Default)]
pub struct SnapshotBackend {
    // The cache is disk-backed. Serialize requests instead of retaining a
    // gigabyte catalog per drive or running two full C scans simultaneously.
    serial: Arc<AtomicBool>,
}
struct Permit(Arc<AtomicBool>);
enum SnapshotData {
    Full(snapshot::Snapshot),
    Cached(crate::administration::cache::CachedSnapshot),
}
impl SnapshotData {
    fn visit(
        &mut self,
        drive: prism_ntfs_protocol::DriveLetter,
        check: &impl Fn() -> layout::Result<()>,
        visit: impl FnMut(prism_ntfs_protocol::FileRecord) -> io::Result<()>,
    ) -> io::Result<()> {
        match self {
            Self::Full(snapshot) => snapshot.catalog.visit(drive, visit),
            Self::Cached(snapshot) => snapshot.visit(drive, check, visit),
        }
    }
    fn save(
        self,
        store: &crate::administration::cache::Store,
        device: &str,
        check: impl Fn() -> layout::Result<()>,
    ) -> io::Result<()> {
        match self {
            Self::Full(snapshot) => store.save(&snapshot, device, check),
            Self::Cached(snapshot) => store.save_cached(snapshot, device, check),
        }
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
fn acquire(
    busy: &Arc<AtomicBool>,
    check: &impl Fn() -> layout::Result<()>,
) -> layout::Result<Permit> {
    loop {
        check()?;
        if busy
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            return Ok(Permit(busy.clone()));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}
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
        let emit = RefCell::new(emit);
        let completed = Cell::new(0);
        let last = Cell::new(Instant::now());
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
                // Includes waiting for another request, hashing/loading and
                // saving cache files. None may silently exceed the client's
                // frame deadline while the overall request remains live.
                if last.get().elapsed() >= Duration::from_millis(500) {
                    if emit.borrow_mut()(Response::Progress {
                        completed: completed.get(),
                    })
                    .is_err()
                    {
                        cancel.store(true, Ordering::Release);
                        return Err(Error::Cancelled);
                    }
                    last.set(Instant::now());
                }
                Ok(())
            }
        };
        check().map_err(failure)?;
        let serial = acquire(&self.serial, &check).map_err(failure)?;
        let mut volume =
            Volume::open(drive, cancel.clone()).map_err(|error| failure(error.into()))?;
        let geometry = volume.geometry();
        let device = volume
            .device_name()
            .map_err(|error| failure(error.into()))?;
        emit.borrow_mut()(Response::Volume {
            serial: geometry.serial,
            device: device.clone(),
        })
        .map_err(|_| FailureCode::Cancelled)?;
        let progress = |progress| {
            completed.set(match progress {
                snapshot::Progress::Mft { processed, .. } => processed,
                snapshot::Progress::Replay { processed, .. } => {
                    geometry.mft_bytes / u64::from(geometry.record_size) + processed
                }
            });
        };
        let store = crate::administration::cache::Store::open(drive).ok();
        let cached = store
            .as_ref()
            .and_then(|store| store.load(geometry, &device, check).ok().flatten());
        // A cancelled or timed-out load must not turn into a new bulk scan.
        check().map_err(failure)?;
        let refreshed =
            cached.map(|cached| cached.refresh(&mut volume, geometry, check, &progress));
        let mut result = match refreshed {
            Some(Ok(result)) => SnapshotData::Cached(result),
            Some(Err(Error::Cancelled)) => return Err(FailureCode::Cancelled),
            Some(Err(Error::TimedOut)) => return Err(FailureCode::Unavailable),
            // Wrapped/lost journals, incompatible caches and inconsistent
            // metadata families invalidate private state and require a fresh
            // read. Never send paths from the discarded partial refresh.
            _ => SnapshotData::Full(
                snapshot::collect(&mut volume, geometry, check, &progress).map_err(failure)?,
            ),
        };
        check().map_err(failure)?;
        let mut batch = vec![];
        let mut encoded = 5usize;
        result
            .visit(drive, &check, |record| -> io::Result<()> {
                check().map_err(io::Error::other)?;
                let length = record.path.len() + 22;
                if !batch.is_empty()
                    && (batch.len() == MAX_BATCH_RECORDS
                        || encoded.saturating_add(length) > MAX_FRAME_BYTES)
                {
                    emit.borrow_mut()(Response::Records(std::mem::take(&mut batch)))
                        .map_err(|_| io::Error::from(io::ErrorKind::Interrupted))?;
                    encoded = 5;
                }
                encoded += length;
                batch.push(record);
                Ok(())
            })
            .map_err(visitor_failure)?;
        if !batch.is_empty() {
            emit.borrow_mut()(Response::Records(batch)).map_err(|_| FailureCode::Cancelled)?;
        }
        check().map_err(failure)?;
        if let Some(store) = store {
            // Commit after transfer, without delaying terminal acknowledgement
            // for a full cache rewrite. Move the one catalog and serialization
            // permit together: the next request cannot allocate another full
            // catalog until this writer has released its memory and handles.
            let _ = std::thread::Builder::new()
                .name("prism-cache-save".into())
                .spawn(move || {
                    let begin = Instant::now();
                    let check = || {
                        if shutdown.load(Ordering::Acquire) {
                            Err(Error::Cancelled)
                        } else if begin.elapsed() > Duration::from_secs(30) {
                            Err(Error::TimedOut)
                        } else {
                            Ok(())
                        }
                    };
                    // Session cancellation is intentionally NOT used: the session
                    // sets its flag even after a successful terminal ACK. Service
                    // shutdown and this writer's own deadline still apply.
                    let _ = result.save(&store, &device, check);
                    drop(store);
                    drop(serial);
                });
        }
        Ok((SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| FailureCode::Unavailable)?
            .as_nanos() as u64)
            .max(1))
    }
}
fn visitor_failure(error: io::Error) -> FailureCode {
    if let Some(error) = error
        .get_ref()
        .and_then(|error| error.downcast_ref::<layout::Error>())
    {
        failure(*error)
    } else {
        failure(error.into())
    }
}
fn failure(error: layout::Error) -> FailureCode {
    match error {
        Error::Cancelled => FailureCode::Cancelled,
        Error::JournalGap | Error::Changed | Error::Inconsistent => FailureCode::JournalGap,
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
    #[test]
    fn streaming_visitor_keeps_cancellation_deadline_and_volume_failure_codes() {
        for (error, expected) in [
            (Error::Cancelled, FailureCode::Cancelled),
            (Error::TimedOut, FailureCode::Unavailable),
            (Error::JournalGap, FailureCode::JournalGap),
            (Error::Corrupt, FailureCode::CorruptVolume),
        ] {
            assert_eq!(visitor_failure(io::Error::other(error)), expected);
        }
        assert_eq!(
            visitor_failure(io::ErrorKind::Interrupted.into()),
            FailureCode::Cancelled
        );
        assert_eq!(
            visitor_failure(io::ErrorKind::PermissionDenied.into()),
            FailureCode::AccessDenied
        );
    }
    #[test]
    fn deferred_commit_owns_serialization_until_its_thread_releases_memory() {
        let busy = Arc::new(AtomicBool::new(false));
        let permit = acquire(&busy, &|| Ok(())).unwrap();
        let (release, wait) = std::sync::mpsc::channel();
        let writer = std::thread::spawn(move || {
            let _permit = permit;
            wait.recv().unwrap();
        });
        assert!(busy.load(Ordering::Acquire));
        assert!(matches!(
            acquire(&busy, &|| Err(Error::Cancelled)),
            Err(Error::Cancelled)
        ));
        assert!(
            busy.load(Ordering::Acquire),
            "cancelling a waiter must not release the writer's permit"
        );
        release.send(()).unwrap();
        writer.join().unwrap();
        let next = acquire(&busy, &|| Ok(())).unwrap();
        drop(next);
        assert!(!busy.load(Ordering::Acquire));
    }
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
            emit(Response::Volume {
                serial: 7,
                device: r"\Device\TestVolume".into(),
            })
            .unwrap();
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
                r"\Device\TestVolume",
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
            r"\Device\TestVolume",
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

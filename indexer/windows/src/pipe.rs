//! Local byte-stream transport. Only `connect` is a public client constructor;
//! it authenticates the peer against SCM before sending a request. Unit tests
//! use private names/constructors and do not register a pretend Windows service.
use crate::{scm::PeerIdentity, security::Descriptor};
use std::{
    io::{self, Read, Write},
    mem::size_of,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    ptr::{null, null_mut},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::*,
    Security::*,
    Storage::FileSystem::*,
    System::{Pipes::*, Threading::*, IO::*},
};

pub const PIPE_NAME: &str = r"\\.\pipe\Prism.FileIndex.v2";
const MAX_OPERATION: Duration = Duration::from_secs(120);
const CLIENT_RIGHTS: u32 = 0x0012_0083;
const PIPE_SDDL: &str = "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;OW)(A;;0x120083;;;IU)";
const MAX_INSTANCES: u32 = 8;

fn cancelled() -> io::Error {
    io::Error::new(
        // read_exact/write_all automatically retry Interrupted. Persistent
        // cancellation must terminate those loops rather than busy-spin.
        io::ErrorKind::ConnectionAborted,
        "Filename index connection cancelled.",
    )
}
fn timeout() -> io::Error {
    io::Error::new(
        io::ErrorKind::TimedOut,
        "Filename index connection timed out.",
    )
}
fn terminated(cancel: &AtomicBool, shutdown: &AtomicBool) -> bool {
    cancel.load(Ordering::Acquire) || shutdown.load(Ordering::Acquire)
}

fn perform(
    handle: HANDLE,
    cancel: &AtomicBool,
    shutdown: &AtomicBool,
    deadline: Instant,
    start: impl FnOnce(*mut OVERLAPPED) -> i32,
) -> io::Result<usize> {
    if terminated(cancel, shutdown) {
        return Err(cancelled());
    }
    if Instant::now() >= deadline {
        return Err(timeout());
    }
    // SAFETY: one owned, unnamed, non-inheritable completion event.
    let event = unsafe { CreateEventW(null(), 1, 0, null()) };
    if event.is_null() {
        return Err(io::Error::last_os_error());
    }
    let event = unsafe { OwnedHandle::from_raw_handle(event) };
    let mut operation = Box::new(OVERLAPPED::default());
    operation.hEvent = event.as_raw_handle();
    let pending = &mut *operation as *mut OVERLAPPED;
    if start(pending) == 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(ERROR_IO_PENDING as i32) {
            return Err(error);
        }
    }
    let failure = loop {
        if terminated(cancel, shutdown) {
            break Some(cancelled());
        }
        if Instant::now() >= deadline {
            break Some(timeout());
        }
        // SAFETY: the operation, event and all borrowed I/O buffers remain
        // alive until completion or cancellation drain below.
        match unsafe { WaitForSingleObject(event.as_raw_handle(), 20) } {
            WAIT_OBJECT_0 => break None,
            WAIT_TIMEOUT => {}
            _ => break Some(io::Error::last_os_error()),
        }
    };
    let mut transferred = 0;
    if let Some(error) = failure {
        unsafe {
            CancelIoEx(handle, pending);
            GetOverlappedResult(handle, pending, &mut transferred, 1);
        }
        return Err(error);
    }
    if unsafe { GetOverlappedResult(handle, pending, &mut transferred, 0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(transferred as usize)
}

pub struct Connection {
    handle: OwnedHandle,
    server: bool,
    cancel: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    absolute: Instant,
    deadline: Instant,
    _identity: Option<PeerIdentity>,
}
impl Connection {
    pub fn connect(cancel: Arc<AtomicBool>) -> io::Result<Self> {
        let identity = PeerIdentity::running()?;
        let mut connection = Self::open_named(PIPE_NAME, cancel)?;
        identity.verify_pipe(connection.handle.as_raw_handle())?;
        connection._identity = Some(identity);
        Ok(connection)
    }
    fn open_named(name: &str, cancel: Arc<AtomicBool>) -> io::Result<Self> {
        let wide: Vec<_> = name.encode_utf16().chain(Some(0)).collect();
        let begun = Instant::now();
        let raw = loop {
            if cancel.load(Ordering::Acquire) {
                return Err(cancelled());
            }
            // Specific rights omit FILE_CREATE_PIPE_INSTANCE. Identification
            // SQOS prevents this server from impersonating its client.
            let raw = unsafe {
                CreateFileW(
                    wide.as_ptr(),
                    CLIENT_RIGHTS,
                    0,
                    null(),
                    OPEN_EXISTING,
                    FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                    null_mut(),
                )
            };
            if raw != INVALID_HANDLE_VALUE {
                break raw;
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(ERROR_PIPE_BUSY as i32) {
                return Err(error);
            }
            if begun.elapsed() >= Duration::from_millis(250) {
                return Err(timeout());
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let absolute = Instant::now() + MAX_OPERATION;
        Ok(Self {
            handle: unsafe { OwnedHandle::from_raw_handle(raw) },
            server: false,
            cancel,
            shutdown: Arc::new(AtomicBool::new(false)),
            absolute,
            deadline: Instant::now() + Duration::from_secs(2),
            _identity: None,
        })
    }
    #[cfg(test)]
    pub(crate) fn fixture_client(name: &str) -> io::Result<Self> {
        Self::open_named(name, Arc::new(AtomicBool::new(false)))
    }
    pub fn set_frame_timeout(&mut self, duration: Duration) {
        self.deadline = (Instant::now() + duration.min(MAX_OPERATION)).min(self.absolute);
    }
    pub fn cancel_flag(&self) -> Arc<AtomicBool> {
        self.cancel.clone()
    }
    pub fn cancelled(&self) -> bool {
        terminated(&self.cancel, &self.shutdown)
    }
    pub fn shutdown_flag(&self) -> Arc<AtomicBool> {
        self.shutdown.clone()
    }
    pub fn exchange(
        &mut self,
        request: prism_ntfs_protocol::Request,
        expected_serial: u64,
        expected_device: &str,
        mut receive: impl FnMut(prism_ntfs_protocol::stream::Event) -> io::Result<()>,
    ) -> io::Result<u64> {
        use prism_ntfs_protocol::{
            stream::{Error, Event, Receiver},
            Acknowledgement, Response,
        };
        let drive = match request {
            prism_ntfs_protocol::Request::Snapshot { drive }
            | prism_ntfs_protocol::Request::Changes { drive, .. } => drive,
        };
        self.set_frame_timeout(Duration::from_secs(2));
        request.write(self)?;
        let mut receiver = Receiver::new(request, expected_serial, expected_device);
        loop {
            self.set_frame_timeout(Duration::from_secs(20));
            let response = Response::read(self, drive)?;
            match receiver.accept(response) {
                Ok(Some(Event::Finished { revision })) => {
                    self.set_frame_timeout(Duration::from_secs(2));
                    Acknowledgement::Complete { revision }.write(self)?;
                    return Ok(revision);
                }
                Ok(Some(event)) => receive(event)?,
                Ok(None) => {}
                Err(Error::Service(code)) => {
                    self.set_frame_timeout(Duration::from_secs(2));
                    let _ = Acknowledgement::Failure(code).write(self);
                    return Err(io::Error::other(Error::Service(code)));
                }
                Err(error) => return Err(io::Error::new(io::ErrorKind::InvalidData, error)),
            }
        }
    }
}
impl Read for Connection {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        let length = output.len().min(2 * 1024 * 1024) as u32;
        let pointer = output.as_mut_ptr();
        // SAFETY: perform keeps this borrowed output and OVERLAPPED alive until
        // completion/drain; pipe reads need no sector-aligned buffer.
        match perform(
            self.handle.as_raw_handle(),
            &self.cancel,
            &self.shutdown,
            self.deadline,
            |operation| unsafe {
                ReadFile(
                    self.handle.as_raw_handle(),
                    pointer,
                    length,
                    null_mut(),
                    operation,
                )
            },
        ) {
            Err(error) if error.raw_os_error() == Some(ERROR_BROKEN_PIPE as i32) => Ok(0),
            result => result,
        }
    }
}
impl Write for Connection {
    fn write(&mut self, input: &[u8]) -> io::Result<usize> {
        if input.is_empty() {
            return Ok(0);
        }
        let length = input.len().min(2 * 1024 * 1024) as u32;
        let pointer = input.as_ptr();
        // SAFETY: input is retained until completion/drain in perform.
        perform(
            self.handle.as_raw_handle(),
            &self.cancel,
            &self.shutdown,
            self.deadline,
            |operation| unsafe {
                WriteFile(
                    self.handle.as_raw_handle(),
                    pointer,
                    length,
                    null_mut(),
                    operation,
                )
            },
        )
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        // Never FlushFileBuffers: it can wait forever for a client to read. The
        // protocol acknowledgement confirms consumption before a normal shutdown.
        if self.server {
            unsafe {
                DisconnectNamedPipe(self.handle.as_raw_handle());
            }
        }
    }
}

pub struct Server {
    name: Vec<u16>,
    descriptor: Descriptor,
    listener: OwnedHandle,
}
impl Server {
    pub fn new() -> io::Result<Self> {
        Self::named(PIPE_NAME)
    }
    fn named(name: &str) -> io::Result<Self> {
        let descriptor = Descriptor::parse(PIPE_SDDL)?;
        let name: Vec<_> = name.encode_utf16().chain(Some(0)).collect();
        let listener = Self::create(&name, &descriptor, true)?;
        Ok(Self {
            name,
            descriptor,
            listener,
        })
    }
    #[cfg(test)]
    pub(crate) fn fixture(name: &str) -> io::Result<Self> {
        Self::named(name)
    }
    fn create(name: &[u16], descriptor: &Descriptor, first: bool) -> io::Result<OwnedHandle> {
        let security = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.raw(),
            bInheritHandle: 0,
        };
        // Reject network clients, keep a protected explicit DACL, and reserve
        // the first instance so an existing impostor cannot be joined.
        let raw = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_DUPLEX
                    | FILE_FLAG_OVERLAPPED
                    | if first {
                        FILE_FLAG_FIRST_PIPE_INSTANCE
                    } else {
                        0
                    },
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                MAX_INSTANCES,
                65536,
                65536,
                1000,
                &security,
            )
        };
        if raw == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { OwnedHandle::from_raw_handle(raw) })
    }
    pub fn accept(&mut self, shutdown: Arc<AtomicBool>, wait: Duration) -> io::Result<Connection> {
        let cancel = Arc::new(AtomicBool::new(false));
        let deadline = Instant::now() + wait.min(MAX_OPERATION);
        let connected = perform(
            self.listener.as_raw_handle(),
            &cancel,
            &shutdown,
            deadline,
            |operation| unsafe { ConnectNamedPipe(self.listener.as_raw_handle(), operation) },
        );
        if let Err(error) = connected {
            if error.raw_os_error() != Some(ERROR_PIPE_CONNECTED as i32) {
                return Err(error);
            }
        }
        // Keep one listening handle alive even while all accepted connections
        // close, preserving the name/security owner throughout service life.
        let next = Self::create(&self.name, &self.descriptor, false)?;
        let handle = std::mem::replace(&mut self.listener, next);
        let absolute = Instant::now() + MAX_OPERATION;
        Ok(Connection {
            handle,
            server: true,
            cancel,
            shutdown,
            absolute,
            deadline: Instant::now() + Duration::from_secs(2),
            _identity: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scm;
    use std::sync::atomic::AtomicU64;
    static NEXT: AtomicU64 = AtomicU64::new(0);
    fn name() -> String {
        format!(
            r"\\.\pipe\Prism.indexer.test.{}.{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )
    }
    #[test]
    fn real_pipe_streams_bytes_and_reports_its_actual_server_pid() {
        let name = name();
        let server_name = name.clone();
        let (ready, waiting) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            let mut server = Server::fixture(&server_name).unwrap();
            ready.send(()).unwrap();
            let mut pipe = server
                .accept(Arc::new(AtomicBool::new(false)), Duration::from_secs(3))
                .unwrap();
            let mut request = [0; 4];
            pipe.read_exact(&mut request).unwrap();
            assert_eq!(&request, b"ping");
            pipe.write_all(b"pong").unwrap();
            let mut ack = [0; 1];
            pipe.read_exact(&mut ack).unwrap();
            assert_eq!(ack, [1]);
        });
        waiting.recv().unwrap();
        let mut client = Connection::fixture_client(&name).unwrap();
        scm::verify_pipe_pid(client.handle.as_raw_handle(), std::process::id()).unwrap();
        assert_eq!(
            scm::verify_pipe_pid(client.handle.as_raw_handle(), 0)
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
        client.write_all(b"ping").unwrap();
        let mut output = [0; 4];
        client.read_exact(&mut output).unwrap();
        assert_eq!(&output, b"pong");
        client.write_all(&[1]).unwrap();
        thread.join().unwrap();
    }
    #[test]
    fn cancellation_and_timeout_drain_pending_reads() {
        let name = name();
        let server_name = name.clone();
        let (ready, waiting) = std::sync::mpsc::channel();
        let (done, closing) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            let mut server = Server::named(&server_name).unwrap();
            ready.send(()).unwrap();
            let _pipe = server
                .accept(Arc::new(AtomicBool::new(false)), Duration::from_secs(3))
                .unwrap();
            closing.recv().unwrap();
        });
        waiting.recv().unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let mut client = Connection::open_named(&name, cancel.clone()).unwrap();
        client.set_frame_timeout(Duration::from_millis(50));
        assert_eq!(
            client.read_exact(&mut [0; 1]).unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
        client.set_frame_timeout(Duration::from_secs(2));
        let trigger = cancel.clone();
        let notify = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            trigger.store(true, Ordering::Release);
        });
        assert_eq!(
            client.read_exact(&mut [0; 1]).unwrap_err().kind(),
            io::ErrorKind::ConnectionAborted
        );
        notify.join().unwrap();
        done.send(()).unwrap();
        thread.join().unwrap();
    }
    #[test]
    fn pipe_clients_are_not_granted_create_instance_access() {
        assert_eq!(CLIENT_RIGHTS & FILE_CREATE_PIPE_INSTANCE, 0);
        let descriptor = Descriptor::parse(PIPE_SDDL).unwrap();
        let mut present = 0;
        let mut dacl = null_mut();
        let mut defaulted = 0;
        assert_ne!(
            unsafe {
                GetSecurityDescriptorDacl(descriptor.raw(), &mut present, &mut dacl, &mut defaulted)
            },
            0
        );
        assert_ne!(present, 0);
        assert!(!dacl.is_null());
        // Inspect the parsed interactive-user ACE, rather than assuming the
        // SDDL string has been accepted with the intended access mask.
        let mut ace = null_mut();
        assert_ne!(unsafe { GetAce(dacl, 3, &mut ace) }, 0);
        let mask = unsafe { std::ptr::read_unaligned((ace as *const u8).add(4).cast::<u32>()) };
        assert_eq!(mask, CLIENT_RIGHTS);
        assert_eq!(mask & FILE_CREATE_PIPE_INSTANCE, 0);
    }
    #[test]
    fn a_timed_out_accept_can_be_rearmed_without_replacing_the_service() {
        let name = name();
        let server_name = name.clone();
        let (ready, waiting) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            let mut server = Server::named(&server_name).unwrap();
            let stop = Arc::new(AtomicBool::new(false));
            assert!(
                matches!(server.accept(stop.clone(),Duration::from_millis(30)),Err(error) if error.kind()==io::ErrorKind::TimedOut)
            );
            ready.send(()).unwrap();
            let mut connection = server.accept(stop, Duration::from_secs(2)).unwrap();
            let mut ack = [0; 1];
            connection.read_exact(&mut ack).unwrap();
            assert_eq!(ack, [1]);
        });
        waiting.recv().unwrap();
        let mut connection =
            Connection::open_named(&name, Arc::new(AtomicBool::new(false))).unwrap();
        connection.write_all(&[1]).unwrap();
        thread.join().unwrap();
    }
}

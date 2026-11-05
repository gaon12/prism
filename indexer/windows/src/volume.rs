use prism_ntfs_core::journal;
use prism_ntfs_core::{
    layout::{Error, Geometry, RecordBuffer, Source},
    FileReference,
};
use prism_ntfs_protocol::DriveLetter;
use std::{
    io,
    mem::{offset_of, size_of},
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    ptr::{null, null_mut, NonNull},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::*,
    Storage::FileSystem::*,
    System::{Ioctl::*, Memory::*, Threading::*, IO::*},
};

const BLOCK_BYTES: usize = 4 * 1024 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(15);
// mountmgr.h: CTL_CODE('M', 2, METHOD_BUFFERED, FILE_ANY_ACCESS).
const IOCTL_MOUNTDEV_QUERY_DEVICE_NAME: u32 = ((b'M' as u32) << 16) | (2 << 2);

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Invalid NTFS volume metadata.")
}
fn cancelled() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, "NTFS operation cancelled.")
}

/// VirtualAlloc provides page-aligned storage for direct volume reads. This
/// memory is retained until the OS has completed (or drained cancellation of)
/// the operation that owns it; no borrowed Rust slice is passed to pending I/O.
struct NativeBuffer {
    pointer: NonNull<u8>,
    capacity: usize,
}
impl NativeBuffer {
    fn new(capacity: usize) -> io::Result<Self> {
        if capacity == 0 || capacity > BLOCK_BYTES + 8192 {
            return Err(invalid());
        }
        // SAFETY: request owned, zero-filled, readable/writable committed pages.
        let pointer =
            unsafe { VirtualAlloc(null(), capacity, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE) };
        Ok(Self {
            pointer: NonNull::new(pointer.cast()).ok_or_else(io::Error::last_os_error)?,
            capacity,
        })
    }
    fn bytes(&self, length: usize) -> &[u8] {
        assert!(length <= self.capacity);
        // SAFETY: this allocation is initialized and I/O has already completed
        // before callers inspect it; the slice cannot outlive the allocation.
        unsafe { std::slice::from_raw_parts(self.pointer.as_ptr(), length) }
    }
}
impl Drop for NativeBuffer {
    fn drop(&mut self) {
        // SAFETY: every pending operation is drained before this owner is dropped.
        unsafe {
            VirtualFree(self.pointer.as_ptr().cast(), 0, MEM_RELEASE);
        }
    }
}

struct Device {
    handle: OwnedHandle,
    cancel: Arc<AtomicBool>,
}
impl Device {
    fn open(drive: DriveLetter, cancel: Arc<AtomicBool>) -> io::Result<Self> {
        if cancel.load(Ordering::Acquire) {
            return Err(cancelled());
        }
        let path: Vec<_> = format!("\\\\.\\{}:", drive.byte() as char)
            .encode_utf16()
            .chain(Some(0))
            .collect();
        // SAFETY: a fixed drive-letter volume path, read access only, no
        // inheritance, no mount/dismount/locking or filesystem mutation.
        let raw = unsafe {
            CreateFileW(
                path.as_ptr(),
                GENERIC_READ,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                null(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED | FILE_FLAG_NO_BUFFERING,
                null_mut(),
            )
        };
        if raw == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: CreateFile transferred one valid owned handle.
        Ok(Self {
            handle: unsafe { OwnedHandle::from_raw_handle(raw) },
            cancel,
        })
    }
    fn raw(&self) -> HANDLE {
        self.handle.as_raw_handle()
    }
    fn perform(
        &self,
        buffer: &mut NativeBuffer,
        offset: u64,
        start: impl FnOnce(*mut OVERLAPPED, *mut u8) -> i32,
    ) -> io::Result<usize> {
        if self.cancel.load(Ordering::Acquire) {
            return Err(cancelled());
        }
        // SAFETY: unnamed non-inheritable manual-reset completion event.
        let event = unsafe { CreateEventW(null(), 1, 0, null()) };
        if event.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful CreateEvent transferred one valid handle.
        let event = unsafe { OwnedHandle::from_raw_handle(event) };
        let mut overlapped = Box::new(OVERLAPPED::default());
        overlapped.hEvent = event.as_raw_handle();
        overlapped.Anonymous.Anonymous.Offset = offset as u32;
        overlapped.Anonymous.Anonymous.OffsetHigh = (offset >> 32) as u32;
        let pending = &mut *overlapped as *mut OVERLAPPED;
        let begun = Instant::now();
        if start(pending, buffer.pointer.as_ptr()) == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(ERROR_IO_PENDING as i32) {
                return Err(error);
            }
        }
        let failure = loop {
            if self.cancel.load(Ordering::Acquire) {
                break Some(cancelled());
            }
            if begun.elapsed() >= IO_TIMEOUT {
                break Some(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "NTFS read timed out.",
                ));
            }
            // SAFETY: the completion event and operation stay owned here.
            match unsafe { WaitForSingleObject(event.as_raw_handle(), 25) } {
                WAIT_OBJECT_0 => break None,
                WAIT_TIMEOUT => {}
                _ => break Some(io::Error::last_os_error()),
            }
        };
        let mut transferred = 0;
        if let Some(error) = failure {
            // SAFETY: cancel only this owned operation, then drain it before
            // releasing OVERLAPPED, its input, event or output allocation.
            // A defective driver can delay cancellation cleanup; a service
            // transports must enforce separate pipe deadlines so clients do
            // not depend on a stalled driver's cleanup completing promptly.
            unsafe {
                CancelIoEx(self.raw(), pending);
                GetOverlappedResult(self.raw(), pending, &mut transferred, 1);
            }
            return Err(error);
        }
        // SAFETY: completion was signalled and pointers still refer to live
        // owned storage. Querying the result does not start another operation.
        if unsafe { GetOverlappedResult(self.raw(), pending, &mut transferred, 0) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if transferred as usize > buffer.capacity {
            return Err(invalid());
        }
        Ok(transferred as usize)
    }
    fn control(&self, code: u32, input: &[u8], capacity: usize) -> io::Result<Vec<u8>> {
        let mut output = NativeBuffer::new(capacity)?;
        let transferred = self.perform(&mut output, 0, |overlapped, buffer| {
            // SAFETY: all buffers stay alive until perform completes/drains.
            unsafe {
                DeviceIoControl(
                    self.raw(),
                    code,
                    if input.is_empty() {
                        null()
                    } else {
                        input.as_ptr().cast()
                    },
                    input.len() as u32,
                    buffer.cast(),
                    capacity as u32,
                    null_mut(),
                    overlapped,
                )
            }
        })?;
        Ok(output.bytes(transferred).to_vec())
    }
}

pub struct Volume {
    device: Device,
    geometry: Geometry,
    scratch: NativeBuffer,
}
impl Volume {
    pub fn open(drive: DriveLetter, cancel: Arc<AtomicBool>) -> io::Result<Self> {
        let device = Device::open(drive, cancel)?;
        let data = device.control(FSCTL_GET_NTFS_VOLUME_DATA, &[], 512)?;
        let base_size = size_of::<NTFS_VOLUME_DATA_BUFFER>();
        if data.len() < base_size + 8 {
            return Err(invalid());
        }
        // SAFETY: these are Win32 numeric POD fields, and the full structure
        // was initialized by the successful, completed volume query.
        let base =
            unsafe { std::ptr::read_unaligned(data.as_ptr().cast::<NTFS_VOLUME_DATA_BUFFER>()) };
        let major = u16::from_le_bytes(data[base_size + 4..base_size + 6].try_into().unwrap());
        let minor = u16::from_le_bytes(data[base_size + 6..base_size + 8].try_into().unwrap());
        if major != 3 || minor > 1 {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Unsupported NTFS metadata version.",
            ));
        }
        let sectors = u64::try_from(base.NumberSectors).map_err(|_| invalid())?;
        let geometry = Geometry {
            sector_size: base.BytesPerSector,
            cluster_size: base.BytesPerCluster,
            record_size: base.BytesPerFileRecordSegment,
            volume_bytes: sectors
                .checked_mul(u64::from(base.BytesPerSector))
                .ok_or_else(invalid)?,
            mft_bytes: u64::try_from(base.MftValidDataLength).map_err(|_| invalid())?,
            serial: base.VolumeSerialNumber as u64,
        }
        .validate()
        .map_err(|_| invalid())?;
        Ok(Self {
            device,
            geometry,
            scratch: NativeBuffer::new(BLOCK_BYTES + 8192)?,
        })
    }
    pub fn geometry(&self) -> Geometry {
        self.geometry
    }
    pub fn device_name(&self) -> io::Result<String> {
        device_name(&self.device)
    }
    pub fn cancel_flag(&self) -> Arc<AtomicBool> {
        self.device.cancel.clone()
    }
    pub(crate) fn control(&self, code: u32, input: &[u8], capacity: usize) -> io::Result<Vec<u8>> {
        self.device.control(code, input, capacity)
    }
}

fn device_name(device: &Device) -> io::Result<String> {
    let bytes = device.control(IOCTL_MOUNTDEV_QUERY_DEVICE_NAME, &[], 4096)?;
    if bytes.len() < 2 {
        return Err(invalid());
    }
    let length = u16::from_le_bytes(bytes[..2].try_into().unwrap()) as usize;
    if !length.is_multiple_of(2) || length > bytes.len() - 2 {
        return Err(invalid());
    }
    let units: Vec<_> = bytes[2..2 + length]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|unit| u16::from_le_bytes(*unit))
        .collect();
    let name = String::from_utf16(&units).map_err(|_| invalid())?;
    let name = name.trim_end_matches('\0');
    if !prism_ntfs_protocol::valid_device_name(name) {
        return Err(invalid());
    }
    Ok(name.to_owned())
}
impl Source for Volume {
    fn read_exact_at(&mut self, mut offset: u64, mut output: &mut [u8]) -> Result<(), Error> {
        let end = offset
            .checked_add(output.len() as u64)
            .ok_or(Error::Corrupt)?;
        if end > self.geometry.volume_bytes {
            return Err(Error::Corrupt);
        }
        let sector = u64::from(self.geometry.sector_size);
        while !output.is_empty() {
            let start = offset / sector * sector;
            let prefix = (offset - start) as usize;
            let length = output.len().min(BLOCK_BYTES);
            let native_length = (prefix + length).next_multiple_of(sector as usize);
            let received =
                self.device
                    .perform(&mut self.scratch, start, |overlapped, buffer| {
                        // SAFETY: page-aligned owned output, sector-aligned offset and
                        // length, read-only volume handle, retained by perform.
                        unsafe {
                            ReadFile(
                                self.device.raw(),
                                buffer,
                                native_length as u32,
                                null_mut(),
                                overlapped,
                            )
                        }
                    })?;
            if received < native_length {
                return Err(Error::Io(io::ErrorKind::UnexpectedEof));
            }
            output[..length]
                .copy_from_slice(&self.scratch.bytes(received)[prefix..prefix + length]);
            output = &mut output[length..];
            offset += length as u64;
        }
        Ok(())
    }
    fn file_record(&mut self, reference: FileReference) -> Result<RecordBuffer, Error> {
        let bytes = self.control(
            FSCTL_GET_NTFS_FILE_RECORD,
            &reference.0.to_le_bytes(),
            size_of::<NTFS_FILE_RECORD_OUTPUT_BUFFER>() + self.geometry.record_size as usize,
        )?;
        let offset = offset_of!(NTFS_FILE_RECORD_OUTPUT_BUFFER, FileRecordBuffer);
        if bytes.len() < offset {
            return Err(Error::Corrupt);
        }
        let returned = FileReference(u64::from_le_bytes(bytes[..8].try_into().unwrap()));
        let length = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
        if returned.number() != reference.number() {
            return Err(Error::Changed);
        }
        if length != self.geometry.record_size as usize || bytes.len() < offset + length {
            return Err(Error::Corrupt);
        }
        let record = &bytes[offset..offset + length];
        if reference.sequence() != 0
            && u16::from_le_bytes(record[16..18].try_into().unwrap()) != reference.sequence()
        {
            return Err(Error::Changed);
        }
        Ok(RecordBuffer {
            bytes: record.to_vec(),
            kernel_fixups: true,
        })
    }
}

impl journal::Source for Volume {
    fn checkpoint(&mut self) -> Result<journal::Checkpoint, Error> {
        let data = self
            .control(FSCTL_QUERY_USN_JOURNAL, &[], 128)
            .map_err(journal_error)?;
        if data.len() < size_of::<USN_JOURNAL_DATA_V0>() {
            return Err(Error::Corrupt);
        }
        let read = |offset: usize| u64::from_le_bytes(data[offset..offset + 8].try_into().unwrap());
        journal::Checkpoint {
            id: read(0),
            first: read(8) as i64,
            next: read(16) as i64,
            lowest: read(24) as i64,
        }
        .validate()
    }
    fn read_changes(&mut self, id: u64, from: i64) -> Result<Vec<u8>, Error> {
        if from < 0 {
            return Err(Error::Corrupt);
        }
        // Explicit byte encoding leaves all structure padding zero. Immediate
        // reads cannot wait indefinitely for another file close or USN event.
        let mut input = vec![0; size_of::<READ_USN_JOURNAL_DATA_V1>()];
        input[..8].copy_from_slice(&from.to_le_bytes());
        input[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        input[32..40].copy_from_slice(&id.to_le_bytes());
        input[40..42].copy_from_slice(&2u16.to_le_bytes());
        input[42..44].copy_from_slice(&2u16.to_le_bytes());
        self.control(FSCTL_READ_USN_JOURNAL, &input, journal::MAX_JOURNAL_BATCH)
            .map_err(journal_error)
    }
}
fn journal_error(error: io::Error) -> Error {
    match error.raw_os_error().map(|code| code as u32) {
        Some(
            ERROR_JOURNAL_ENTRY_DELETED
            | ERROR_JOURNAL_DELETE_IN_PROGRESS
            | ERROR_JOURNAL_NOT_ACTIVE,
        ) => Error::JournalGap,
        _ => error.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unprivileged_device_metadata_matches_the_open_root_handle() {
        let root = std::env::var("SystemDrive").expect("Windows system drive") + r"\";
        let identity = crate::client::root_identity(std::path::Path::new(&root)).unwrap();
        let path: Vec<_> = format!(r"\\.\{}:", identity.drive.byte() as char)
            .encode_utf16()
            .chain(Some(0))
            .collect();
        // Zero desired access queries only the mount manager identity; it does
        // not grant raw volume reads or require an elevated test process.
        let raw = unsafe {
            CreateFileW(
                path.as_ptr(),
                0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                null(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                null_mut(),
            )
        };
        assert_ne!(raw, INVALID_HANDLE_VALUE);
        let device = Device {
            handle: unsafe { OwnedHandle::from_raw_handle(raw) },
            cancel: Arc::new(AtomicBool::new(false)),
        };
        assert_eq!(device_name(&device).unwrap(), identity.device);
    }
    #[test]
    fn aligned_buffers_are_bounded_and_page_aligned() {
        let buffer = NativeBuffer::new(4096).unwrap();
        assert_eq!(buffer.pointer.as_ptr() as usize % 4096, 0);
        assert!(buffer.bytes(4096).iter().all(|byte| *byte == 0));
        assert!(NativeBuffer::new(BLOCK_BYTES + 8193).is_err());
    }
    #[test]
    fn cancellation_before_open_never_touches_a_volume() {
        let cancel = Arc::new(AtomicBool::new(true));
        assert!(
            matches!(Volume::open(DriveLetter::new(b'C').unwrap(),cancel),Err(error) if error.kind()==io::ErrorKind::Interrupted)
        );
    }
    #[test]
    fn overlapped_reads_align_native_io_and_copy_only_the_requested_range() {
        use std::os::windows::ffi::OsStrExt;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("read-only-io-fixture.bin");
        let bytes: Vec<_> = (0..8192).map(|index| (index % 251) as u8).collect();
        std::fs::write(&path, &bytes).unwrap();
        let wide: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        // SAFETY: the test opens only its newly created fixture, read-only.
        let raw = unsafe {
            CreateFileW(
                wide.as_ptr(),
                GENERIC_READ,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                null(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED | FILE_FLAG_NO_BUFFERING,
                null_mut(),
            )
        };
        assert_ne!(raw, INVALID_HANDLE_VALUE);
        // SAFETY: the successful call transfers one owned handle to the test.
        let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
        let cancel = Arc::new(AtomicBool::new(false));
        let mut volume = Volume {
            device: Device {
                handle,
                cancel: cancel.clone(),
            },
            geometry: Geometry {
                sector_size: 512,
                cluster_size: 512,
                record_size: 1024,
                volume_bytes: 8192,
                mft_bytes: 1024,
                serial: 0,
            },
            scratch: NativeBuffer::new(BLOCK_BYTES + 8192).unwrap(),
        };
        let mut output = vec![0; 1027];
        volume.read_exact_at(13, &mut output).unwrap();
        assert_eq!(output, bytes[13..1040]);
        assert_eq!(volume.read_exact_at(8190, &mut [0; 4]), Err(Error::Corrupt));
        cancel.store(true, Ordering::Release);
        assert_eq!(volume.read_exact_at(0, &mut [0; 4]), Err(Error::Cancelled));
        assert_eq!(
            std::fs::read(&path).unwrap(),
            bytes,
            "native reads must not mutate their source"
        );
    }
}

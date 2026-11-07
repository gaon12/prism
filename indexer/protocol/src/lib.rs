//! The optional Windows index service exposes filenames and metadata only.
//! No request accepts a path, program, command line, file contents or write.
//! Transport authentication and deadlines belong to the named-pipe adapters.

use std::io::{self, Cursor, Read, Write};
pub mod stream;

pub const VERSION: u16 = 2;
pub const MAX_FRAME_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_BATCH_RECORDS: usize = 2048;
const MAX_PATH_BYTES: usize = 4 * 32767;
const MAGIC: [u8; 4] = *b"PNI2";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DriveLetter(u8);
impl DriveLetter {
    pub fn new(letter: u8) -> io::Result<Self> {
        if !letter.is_ascii_alphabetic() {
            return Err(invalid());
        }
        Ok(Self(letter.to_ascii_uppercase()))
    }
    pub fn byte(self) -> u8 {
        self.0
    }
    pub fn root(self) -> String {
        format!("{}:\\", self.0 as char)
    }
    pub fn contains(self, path: &str) -> bool {
        let bytes = path.as_bytes();
        if bytes.len() < 3
            || bytes.len() > MAX_PATH_BYTES
            || bytes[0].to_ascii_uppercase() != self.0
            || &bytes[1..3] != b":\\"
            || bytes.contains(&0)
            || bytes[3..].contains(&b':')
            || bytes.contains(&b'/')
            // A valid UTF-8 string has at most one UTF-16 unit per byte.
            // Ordinary short paths already satisfy the Windows unit bound;
            // count units only for long multibyte paths that may still fit.
            || (bytes.len() > 32767 && path.encode_utf16().count() > 32767)
        {
            return false;
        }
        // A service response must not smuggle an ADS, UNC/device path, or a
        // lexical escape outside the requested drive into the client catalog.
        path.len() == 3
            || path[3..]
                .split('\\')
                .all(|part| !part.is_empty() && part != "." && part != "..")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Request {
    Snapshot { drive: DriveLetter },
    Changes { drive: DriveLetter, after: u64 },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileRecord {
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified_ms: Option<i64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum FailureCode {
    AccessDenied = 1,
    Unavailable = 2,
    UnsupportedVolume = 3,
    Cancelled = 4,
    InvalidRequest = 5,
    Busy = 6,
    CorruptVolume = 7,
    JournalGap = 8,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Response {
    Volume { serial: u64, device: String },
    Progress { completed: u64 },
    Records(Vec<FileRecord>),
    Removed(Vec<String>),
    Finished { revision: u64 },
    Failure(FailureCode),
}

/// A decoded frame whose paths were checked against one drive by this codec.
/// Private fields prevent callers from marking constructed or changed paths as
/// validated. Stream ordering, volume identity and total budgets remain the
/// receiver's responsibility.
#[derive(Debug)]
pub struct ValidatedResponse {
    drive: DriveLetter,
    response: Response,
    path_count: u64,
    path_bytes: u64,
}

/// The server disconnects only after the client confirms consuming the final
/// frame. This avoids dropping unread named-pipe data or using an unbounded
/// FlushFileBuffers call when a client stops reading.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Acknowledgement {
    Complete { revision: u64 },
    Failure(FailureCode),
}
impl Acknowledgement {
    pub fn write(self, writer: &mut impl Write) -> io::Result<()> {
        let mut payload = vec![127];
        match self {
            Self::Complete { revision } if revision != 0 => {
                payload.push(1);
                payload.extend_from_slice(&revision.to_le_bytes());
            }
            Self::Failure(code) => {
                payload.push(2);
                payload.push(code as u8);
            }
            _ => return Err(invalid()),
        }
        write_frame(writer, &payload)
    }
    pub fn read(reader: &mut impl Read) -> io::Result<Self> {
        let mut payload = read_frame(reader)?;
        if byte(&mut payload)? != 127 {
            return Err(invalid());
        }
        let result = match byte(&mut payload)? {
            1 => {
                let revision = u64_value(&mut payload)?;
                if revision == 0 {
                    return Err(invalid());
                }
                Self::Complete { revision }
            }
            2 => Self::Failure(failure_code(byte(&mut payload)?)?),
            _ => return Err(invalid()),
        };
        finish(&payload)?;
        Ok(result)
    }
}

fn failure_code(code: u8) -> io::Result<FailureCode> {
    Ok(match code {
        1 => FailureCode::AccessDenied,
        2 => FailureCode::Unavailable,
        3 => FailureCode::UnsupportedVolume,
        4 => FailureCode::Cancelled,
        5 => FailureCode::InvalidRequest,
        6 => FailureCode::Busy,
        7 => FailureCode::CorruptVolume,
        8 => FailureCode::JournalGap,
        _ => return Err(invalid()),
    })
}

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "Invalid filename index protocol message.",
    )
}

pub fn valid_device_name(value: &str) -> bool {
    value.len() <= 1024
        && !value.contains(['\0', '/'])
        && value.strip_prefix(r"\Device\").is_some_and(|tail| {
            !tail.is_empty()
                && tail
                    .split('\\')
                    .all(|part| !part.is_empty() && part != "." && part != "..")
        })
}

fn write_frame(writer: &mut impl Write, payload: &[u8]) -> io::Result<()> {
    if payload.is_empty() || payload.len() > MAX_FRAME_BYTES {
        return Err(invalid());
    }
    writer.write_all(&MAGIC)?;
    writer.write_all(&VERSION.to_le_bytes())?;
    writer.write_all(&(payload.len() as u32).to_le_bytes())?;
    writer.write_all(payload)
}

fn read_frame(reader: &mut impl Read) -> io::Result<Cursor<Vec<u8>>> {
    let mut header = [0; 10];
    reader.read_exact(&mut header)?;
    let length = u32::from_le_bytes(header[6..10].try_into().unwrap()) as usize;
    if header[..4] != MAGIC
        || u16::from_le_bytes(header[4..6].try_into().unwrap()) != VERSION
        || length == 0
        || length > MAX_FRAME_BYTES
    {
        return Err(invalid());
    }
    // Validate the declared size before allocating or reading a peer's body.
    let mut payload = vec![0; length];
    reader.read_exact(&mut payload)?;
    Ok(Cursor::new(payload))
}

fn byte(reader: &mut impl Read) -> io::Result<u8> {
    let mut value = [0];
    reader.read_exact(&mut value)?;
    Ok(value[0])
}
fn u64_value(reader: &mut impl Read) -> io::Result<u64> {
    let mut value = [0; 8];
    reader.read_exact(&mut value)?;
    Ok(u64::from_le_bytes(value))
}
fn count(reader: &mut impl Read) -> io::Result<usize> {
    let mut value = [0; 4];
    reader.read_exact(&mut value)?;
    let count = u32::from_le_bytes(value) as usize;
    if count > MAX_BATCH_RECORDS {
        return Err(invalid());
    }
    Ok(count)
}
fn finish(cursor: &Cursor<Vec<u8>>) -> io::Result<()> {
    if cursor.position() as usize != cursor.get_ref().len() {
        return Err(invalid());
    }
    Ok(())
}
fn write_path(payload: &mut Vec<u8>, path: &str, drive: DriveLetter) -> io::Result<()> {
    if !drive.contains(path) || payload.len().saturating_add(4 + path.len()) > MAX_FRAME_BYTES {
        return Err(invalid());
    }
    payload.extend_from_slice(&(path.len() as u32).to_le_bytes());
    payload.extend_from_slice(path.as_bytes());
    Ok(())
}
fn read_path(cursor: &mut Cursor<Vec<u8>>, drive: DriveLetter) -> io::Result<String> {
    let mut length = [0; 4];
    cursor.read_exact(&mut length)?;
    let length = u32::from_le_bytes(length) as usize;
    let start = cursor.position() as usize;
    if length > MAX_PATH_BYTES || length > cursor.get_ref().len().saturating_sub(start) {
        return Err(invalid());
    }
    let path =
        std::str::from_utf8(&cursor.get_ref()[start..start + length]).map_err(|_| invalid())?;
    if !drive.contains(path) {
        return Err(invalid());
    }
    let result = path.to_owned();
    cursor.set_position((start + length) as u64);
    Ok(result)
}

impl Request {
    pub fn write(self, writer: &mut impl Write) -> io::Result<()> {
        let payload = match self {
            Self::Snapshot { drive } => vec![1, drive.byte()],
            Self::Changes { drive, after } => {
                let mut payload = vec![2, drive.byte()];
                payload.extend_from_slice(&after.to_le_bytes());
                payload
            }
        };
        write_frame(writer, &payload)
    }
    pub fn read(reader: &mut impl Read) -> io::Result<Self> {
        let mut payload = read_frame(reader)?;
        let kind = byte(&mut payload)?;
        let drive = DriveLetter::new(byte(&mut payload)?)?;
        let request = match kind {
            1 => Self::Snapshot { drive },
            2 => Self::Changes {
                drive,
                after: u64_value(&mut payload)?,
            },
            _ => return Err(invalid()),
        };
        finish(&payload)?;
        Ok(request)
    }
}

impl Response {
    /// Decode with all ordinary frame/path checks, retaining that validation
    /// for a stream receiver without scanning the same paths a second time.
    pub fn read_validated(
        reader: &mut impl Read,
        drive: DriveLetter,
    ) -> io::Result<ValidatedResponse> {
        let response = Self::read(reader, drive)?;
        let (path_count, path_bytes) = match &response {
            Self::Records(records) => (
                records.len() as u64,
                records.iter().map(|row| row.path.len() as u64).sum(),
            ),
            Self::Removed(paths) => (
                paths.len() as u64,
                paths.iter().map(|path| path.len() as u64).sum(),
            ),
            _ => (0, 0),
        };
        // These sums cannot overflow: a decoded frame is at most 2 MiB and
        // contains at most MAX_BATCH_RECORDS paths.
        Ok(ValidatedResponse {
            drive,
            response,
            path_count,
            path_bytes,
        })
    }

    pub fn write(&self, writer: &mut impl Write, drive: DriveLetter) -> io::Result<()> {
        let mut payload = vec![];
        match self {
            Self::Volume { serial, device } => {
                if !valid_device_name(device) {
                    return Err(invalid());
                }
                payload.push(6);
                payload.extend_from_slice(&serial.to_le_bytes());
                payload.extend_from_slice(&(device.len() as u16).to_le_bytes());
                payload.extend_from_slice(device.as_bytes());
            }
            Self::Progress { completed } => {
                payload.push(1);
                payload.extend_from_slice(&completed.to_le_bytes());
            }
            Self::Records(records) => {
                if records.len() > MAX_BATCH_RECORDS {
                    return Err(invalid());
                }
                payload.push(2);
                payload.extend_from_slice(&(records.len() as u32).to_le_bytes());
                for record in records {
                    write_path(&mut payload, &record.path, drive)?;
                    payload.push(u8::from(record.is_dir));
                    payload.extend_from_slice(&record.size.to_le_bytes());
                    payload.push(u8::from(record.modified_ms.is_some()));
                    if let Some(modified) = record.modified_ms {
                        payload.extend_from_slice(&modified.to_le_bytes());
                    }
                    if payload.len() > MAX_FRAME_BYTES {
                        return Err(invalid());
                    }
                }
            }
            Self::Removed(paths) => {
                if paths.len() > MAX_BATCH_RECORDS {
                    return Err(invalid());
                }
                payload.push(3);
                payload.extend_from_slice(&(paths.len() as u32).to_le_bytes());
                for path in paths {
                    write_path(&mut payload, path, drive)?;
                }
            }
            Self::Finished { revision } => {
                payload.push(4);
                payload.extend_from_slice(&revision.to_le_bytes());
            }
            Self::Failure(code) => {
                payload.push(5);
                payload.push(*code as u8);
            }
        }
        write_frame(writer, &payload)
    }
    pub fn read(reader: &mut impl Read, drive: DriveLetter) -> io::Result<Self> {
        let mut payload = read_frame(reader)?;
        let response = match byte(&mut payload)? {
            6 => {
                let serial = u64_value(&mut payload)?;
                let mut length = [0; 2];
                payload.read_exact(&mut length)?;
                let length = u16::from_le_bytes(length) as usize;
                if length > 1024 {
                    return Err(invalid());
                }
                let mut bytes = vec![0; length];
                payload.read_exact(&mut bytes)?;
                let device = String::from_utf8(bytes).map_err(|_| invalid())?;
                if !valid_device_name(&device) {
                    return Err(invalid());
                }
                Self::Volume { serial, device }
            }
            1 => Self::Progress {
                completed: u64_value(&mut payload)?,
            },
            2 => {
                let count = count(&mut payload)?;
                let mut records = Vec::with_capacity(count);
                for _ in 0..count {
                    let path = read_path(&mut payload, drive)?;
                    let is_dir = match byte(&mut payload)? {
                        0 => false,
                        1 => true,
                        _ => return Err(invalid()),
                    };
                    let size = u64_value(&mut payload)?;
                    let modified_ms = match byte(&mut payload)? {
                        0 => None,
                        1 => Some(u64_value(&mut payload)? as i64),
                        _ => return Err(invalid()),
                    };
                    records.push(FileRecord {
                        path,
                        is_dir,
                        size,
                        modified_ms,
                    });
                }
                Self::Records(records)
            }
            3 => {
                let count = count(&mut payload)?;
                let mut paths = Vec::with_capacity(count);
                for _ in 0..count {
                    paths.push(read_path(&mut payload, drive)?);
                }
                Self::Removed(paths)
            }
            4 => Self::Finished {
                revision: u64_value(&mut payload)?,
            },
            5 => Self::Failure(failure_code(byte(&mut payload)?)?),
            _ => return Err(invalid()),
        };
        finish(&payload)?;
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn drive() -> DriveLetter {
        DriveLetter::new(b'c').unwrap()
    }
    #[test]
    fn path_length_uses_utf16_units_without_rejecting_long_multibyte_paths() {
        for tail in ["a".repeat(32764), "가".repeat(32764), "😀".repeat(16382)] {
            let path = format!("C:\\{tail}");
            assert_eq!(path.encode_utf16().count(), 32767);
            assert!(drive().contains(&path));
            assert!(!drive().contains(&(path + "a")));
        }
        assert!(drive().contains("C:\\문서\\가 😀.txt"));
        assert!(!drive().contains("C:\\문서\\..\\escape.txt"));
    }

    #[test]
    fn validated_decode_still_rejects_forged_paths_bad_utf8_and_oversized_frames() {
        let mut valid = Vec::new();
        Response::Records(vec![FileRecord {
            path: "C:\\문서.txt".into(),
            is_dir: false,
            size: 42,
            modified_ms: Some(-1),
        }])
        .write(&mut valid, drive())
        .unwrap();
        assert!(Response::read_validated(&mut Cursor::new(&valid), drive()).is_ok());
        // Ten frame-header bytes, type, record count and the path length.
        let mut wrong_drive = valid.clone();
        wrong_drive[19] = b'D';
        let mut bad_utf8 = valid.clone();
        bad_utf8[22] = 0xff;
        let mut oversized = valid.clone();
        oversized[6..10].copy_from_slice(&((MAX_FRAME_BYTES + 1) as u32).to_le_bytes());
        for bytes in [wrong_drive, bad_utf8, oversized] {
            assert_eq!(
                Response::read_validated(&mut Cursor::new(bytes), drive())
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::InvalidData
            );
        }
        valid.pop();
        assert!(Response::read_validated(&mut Cursor::new(valid), drive()).is_err());
    }
    #[test]
    fn requests_only_identify_local_drive_letters_and_revisions() {
        for request in [
            Request::Snapshot { drive: drive() },
            Request::Changes {
                drive: drive(),
                after: u64::MAX,
            },
        ] {
            let mut bytes = vec![];
            request.write(&mut bytes).unwrap();
            assert_eq!(Request::read(&mut Cursor::new(bytes)).unwrap(), request);
        }
        for value in [0, b'/', b':', b'1', 255] {
            assert!(DriveLetter::new(value).is_err());
        }
        assert_eq!(drive().root(), "C:\\");
    }
    #[test]
    fn completion_acknowledgements_cannot_be_mistaken_for_new_requests() {
        for ack in [
            Acknowledgement::Complete { revision: 42 },
            Acknowledgement::Failure(FailureCode::Busy),
        ] {
            let mut bytes = vec![];
            ack.write(&mut bytes).unwrap();
            assert_eq!(
                Acknowledgement::read(&mut Cursor::new(&bytes)).unwrap(),
                ack
            );
            assert!(Request::read(&mut Cursor::new(bytes)).is_err());
        }
        assert!(Acknowledgement::Complete { revision: 0 }
            .write(&mut vec![])
            .is_err());
        let mut bytes = vec![];
        Request::Snapshot { drive: drive() }
            .write(&mut bytes)
            .unwrap();
        assert!(Acknowledgement::read(&mut Cursor::new(bytes)).is_err());
    }
    #[test]
    fn filenames_and_metadata_round_trip_without_unicode_normalization() {
        let records = vec![
            FileRecord {
                path: "C:\\문서\\가.md".into(),
                is_dir: false,
                size: 98,
                modified_ms: Some(-1),
            },
            FileRecord {
                path: "C:\\문서\\가.md".into(),
                is_dir: false,
                size: 0,
                modified_ms: None,
            },
            FileRecord {
                path: "C:\\".into(),
                is_dir: true,
                size: 0,
                modified_ms: Some(i64::MAX),
            },
        ];
        for message in [
            Response::Volume {
                serial: 0xfedc_ba98_7654_3210,
                device: r"\Device\TestVolume".into(),
            },
            Response::Records(records),
            Response::Removed(vec!["C:\\gone.txt".into()]),
            Response::Progress {
                completed: 2_000_000,
            },
            Response::Finished { revision: 42 },
            Response::Failure(FailureCode::JournalGap),
        ] {
            let mut bytes = vec![];
            message.write(&mut bytes, drive()).unwrap();
            assert_eq!(
                Response::read(&mut Cursor::new(bytes), drive()).unwrap(),
                message
            );
        }
    }
    #[test]
    fn reject_outside_drive_device_stream_and_traversal_paths() {
        for path in [
            "D:\\other",
            "\\\\host\\share",
            "\\\\?\\C:\\device",
            "C:\\a\\..\\x",
            "C:\\a\\.\\x",
            "C:\\a:stream",
            "C:/a",
            "C:\\a\0secret",
            "C:\\a\\",
            "C:\\\\a",
        ] {
            assert!(!drive().contains(path), "{path:?}");
        }
        for path in ["C:\\", "C:\\문서\\file.txt", "c:\\a-b\\x"] {
            assert!(drive().contains(path));
        }
    }
    #[test]
    fn volume_headers_require_a_bounded_kernel_device_name() {
        for device in [
            "",
            "C:\\",
            r"\\Device\Volume",
            r"\Device\",
            r"\Device\..",
            "\\Device\\bad\0name",
        ] {
            assert!(!valid_device_name(device));
            assert!(Response::Volume {
                serial: 7,
                device: device.into()
            }
            .write(&mut vec![], drive())
            .is_err());
        }
        let device = format!("\\Device\\{}", "a".repeat(1024));
        assert!(!valid_device_name(&device));
        let mut bytes = vec![];
        Response::Volume {
            serial: 7,
            device: r"\Device\Volume".into(),
        }
        .write(&mut bytes, drive())
        .unwrap();
        // Both decoding and encoding validate the name, including hostile peers.
        *bytes.last_mut().unwrap() = 0;
        assert!(Response::read(&mut Cursor::new(bytes), drive()).is_err());
    }
    #[test]
    fn reject_oversized_unknown_trailing_and_truncated_messages() {
        let mut bytes = vec![];
        Request::Snapshot { drive: drive() }
            .write(&mut bytes)
            .unwrap();
        for length in 0..bytes.len() {
            assert!(Request::read(&mut Cursor::new(&bytes[..length])).is_err());
        }
        let mut bad = bytes.clone();
        bad[6..10].copy_from_slice(&((MAX_FRAME_BYTES + 1) as u32).to_le_bytes());
        assert!(Request::read(&mut Cursor::new(bad)).is_err());
        let mut bad = bytes.clone();
        bad[4] = VERSION as u8 + 1;
        assert!(Request::read(&mut Cursor::new(bad)).is_err());
        let mut bad = bytes;
        bad.push(0);
        bad[6..10].copy_from_slice(&3u32.to_le_bytes());
        assert!(Request::read(&mut Cursor::new(bad)).is_err());
        let record = FileRecord {
            path: "C:\\a".into(),
            is_dir: false,
            size: 0,
            modified_ms: None,
        };
        assert!(Response::Records(vec![record; MAX_BATCH_RECORDS + 1])
            .write(&mut vec![], drive())
            .is_err());
    }
}

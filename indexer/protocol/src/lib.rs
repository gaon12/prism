//! The optional Windows index service exposes filenames and metadata only.
//! No request accepts a path, program, command line, file contents or write.
//! Transport authentication and deadlines belong to the named-pipe adapters.

use std::io::{self, Cursor, Read, Write};

pub const VERSION: u16 = 1;
pub const MAX_FRAME_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_BATCH_RECORDS: usize = 2048;
const MAX_PATH_BYTES: usize = 4 * 32767;
const MAGIC: [u8; 4] = *b"PNI1";

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
            || (path.is_ascii() && bytes.len() > 32767)
            || (!path.is_ascii() && path.encode_utf16().count() > 32767)
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
    Progress { completed: u64 },
    Records(Vec<FileRecord>),
    Removed(Vec<String>),
    Finished { revision: u64 },
    Failure(FailureCode),
}

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "Invalid filename index protocol message.",
    )
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
    pub fn write(&self, writer: &mut impl Write, drive: DriveLetter) -> io::Result<()> {
        let mut payload = vec![];
        match self {
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
            5 => Self::Failure(match byte(&mut payload)? {
                1 => FailureCode::AccessDenied,
                2 => FailureCode::Unavailable,
                3 => FailureCode::UnsupportedVolume,
                4 => FailureCode::Cancelled,
                5 => FailureCode::InvalidRequest,
                6 => FailureCode::Busy,
                7 => FailureCode::CorruptVolume,
                8 => FailureCode::JournalGap,
                _ => return Err(invalid()),
            }),
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
        bad[4] = 2;
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

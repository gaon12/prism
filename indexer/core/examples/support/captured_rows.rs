//! Developer-only metadata rows, not a trusted service reply or NTFS cache.
//! Callers must keep streamed rows private until read() returns successfully.
use prism_ntfs_protocol::{DriveLetter, FileRecord, Response};
use std::io::{self, Read, Write};
const MAGIC: &[u8; 8] = b"PNROW001";
const MAX_BYTES: u64 = 3 * 1024 * 1024 * 1024;
const MAX_RECORDS: u64 = 10_000_000;
const MAX_PATH_BYTES: u64 = 2 * 1024 * 1024 * 1024;
fn invalid() -> io::Error {
    io::ErrorKind::InvalidData.into()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Summary {
    pub drive: DriveLetter,
    pub records: u64,
    pub path_bytes: u64,
    pub revision: u64,
    pub bytes: u64,
}
impl Summary {
    fn account(&mut self, rows: &[FileRecord]) -> io::Result<()> {
        self.records = self
            .records
            .checked_add(rows.len() as u64)
            .ok_or_else(invalid)?;
        for row in rows {
            self.path_bytes = self
                .path_bytes
                .checked_add(row.path.len() as u64)
                .ok_or_else(invalid)?;
        }
        if self.records > MAX_RECORDS || self.path_bytes > MAX_PATH_BYTES {
            return Err(invalid());
        }
        Ok(())
    }
}
struct Bounded<W> {
    out: W,
    bytes: u64,
}
impl<W: Write> Write for Bounded<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() as u64 > MAX_BYTES.saturating_sub(self.bytes) {
            return Err(invalid());
        }
        let count = self.out.write(bytes)?;
        self.bytes += count as u64;
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }
}
pub struct Writer<W> {
    out: Bounded<W>,
    summary: Summary,
    failed: bool,
}
impl<W: Write> Writer<W> {
    pub fn new(out: W, drive: DriveLetter) -> io::Result<Self> {
        let mut out = Bounded { out, bytes: 0 };
        out.write_all(MAGIC)?;
        out.write_all(&[drive.byte()])?;
        Ok(Self {
            out,
            summary: Summary {
                drive,
                records: 0,
                path_bytes: 0,
                revision: 0,
                bytes: 0,
            },
            failed: false,
        })
    }
    pub fn push(&mut self, rows: Vec<FileRecord>) -> io::Result<()> {
        if self.failed {
            return Err(invalid());
        }
        if rows.is_empty() {
            return Ok(());
        }
        let result = self
            .summary
            .account(&rows)
            .and_then(|()| Response::Records(rows).write(&mut self.out, self.summary.drive));
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    pub fn finish(mut self, revision: u64) -> io::Result<(W, Summary)> {
        if self.failed || revision == 0 {
            return Err(invalid());
        }
        Response::Finished { revision }.write(&mut self.out, self.summary.drive)?;
        self.out.flush()?;
        self.summary.revision = revision;
        self.summary.bytes = self.out.bytes;
        Ok((self.out.out, self.summary))
    }
}
struct Counted<R> {
    input: R,
    bytes: u64,
}
impl<R: Read> Read for Counted<R> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        let remaining = MAX_BYTES.saturating_sub(self.bytes);
        if remaining == 0 {
            return Err(invalid());
        }
        let length = bytes.len().min(remaining as usize);
        let count = self.input.read(&mut bytes[..length])?;
        self.bytes += count as u64;
        Ok(count)
    }
}
pub fn read(
    input: impl Read,
    mut check: impl FnMut() -> io::Result<()>,
    mut visit: impl FnMut(FileRecord) -> io::Result<()>,
) -> io::Result<Summary> {
    let mut input = Counted { input, bytes: 0 };
    let mut magic = [0; 8];
    input.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(invalid());
    }
    let mut letter = [0];
    input.read_exact(&mut letter)?;
    let drive = DriveLetter::new(letter[0])?;
    let mut summary = Summary {
        drive,
        records: 0,
        path_bytes: 0,
        revision: 0,
        bytes: 0,
    };
    loop {
        check()?;
        match Response::read(&mut input, drive)? {
            Response::Records(rows) if !rows.is_empty() => {
                summary.account(&rows)?;
                for row in rows {
                    visit(row)?;
                }
            }
            Response::Finished { revision } if revision != 0 => {
                let mut tail = [0];
                if input.read(&mut tail)? != 0 {
                    return Err(invalid());
                }
                summary.revision = revision;
                summary.bytes = input.bytes;
                return Ok(summary);
            }
            _ => return Err(invalid()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    fn row() -> FileRecord {
        FileRecord {
            path: r"C:\자료-日本語-😀.md".into(),
            is_dir: false,
            size: u64::MAX,
            modified_ms: Some(i64::MIN),
        }
    }
    fn encoded() -> Vec<u8> {
        let mut writer = Writer::new(Vec::new(), DriveLetter::new(b'C').unwrap()).unwrap();
        writer.push(vec![row()]).unwrap();
        writer.finish(73).unwrap().0
    }
    #[test]
    fn complete_rows_preserve_metadata_and_require_terminal_success() {
        let bytes = encoded();
        let mut rows = Vec::new();
        let summary = read(
            Cursor::new(&bytes),
            || Ok(()),
            |row| {
                rows.push(row);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(rows, vec![row()]);
        assert_eq!(summary.records, 1);
        assert_eq!(summary.revision, 73);
        assert_eq!(summary.bytes, bytes.len() as u64);
        for length in 0..bytes.len() {
            assert!(read(Cursor::new(&bytes[..length]), || Ok(()), |_| Ok(())).is_err());
        }
        let mut trailing = bytes;
        trailing.push(0);
        assert!(read(Cursor::new(trailing), || Ok(()), |_| Ok(())).is_err());
    }
    #[test]
    fn failed_writer_and_out_of_scope_rows_cannot_be_finished() {
        let mut writer = Writer::new(Vec::new(), DriveLetter::new(b'C').unwrap()).unwrap();
        let mut outside = row();
        outside.path = r"D:\outside.md".into();
        assert!(writer.push(vec![outside]).is_err());
        assert!(writer.finish(1).is_err());
        let writer = Writer::new(Vec::new(), DriveLetter::new(b'C').unwrap()).unwrap();
        assert!(writer.finish(0).is_err());
        for response in [
            Response::Progress { completed: 1 },
            Response::Removed(vec![r"C:\x".into()]),
            Response::Records(vec![]),
        ] {
            let mut bytes = MAGIC.to_vec();
            bytes.push(b'C');
            response
                .write(&mut bytes, DriveLetter::new(b'C').unwrap())
                .unwrap();
            assert!(read(Cursor::new(bytes), || Ok(()), |_| Ok(())).is_err());
        }
    }
    #[test]
    fn limits_cancel_and_callback_errors_stop_private_capture_processing() {
        let mut out = Bounded {
            out: Vec::new(),
            bytes: MAX_BYTES - 1,
        };
        assert!(out.write_all(&[0, 1]).is_err());
        assert!(out.out.is_empty());
        let mut writer = Writer::new(Vec::new(), DriveLetter::new(b'C').unwrap()).unwrap();
        writer.summary.records = MAX_RECORDS;
        assert!(writer.push(vec![row()]).is_err());
        assert!(writer.finish(1).is_err());
        let bytes = encoded();
        assert_eq!(
            read(
                Cursor::new(&bytes),
                || Err(io::ErrorKind::Interrupted.into()),
                |_| Ok(())
            )
            .unwrap_err()
            .kind(),
            io::ErrorKind::Interrupted
        );
        assert_eq!(
            read(
                Cursor::new(bytes),
                || Ok(()),
                |_| Err(io::ErrorKind::BrokenPipe.into())
            )
            .unwrap_err()
            .kind(),
            io::ErrorKind::BrokenPipe
        );
    }
}

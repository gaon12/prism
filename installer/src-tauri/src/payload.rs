//! The files the setup installs, stored inside the setup `.exe` itself.
//!
//! Format (written by `build.rs`, read here), gzip-compressed:
//! ```text
//! repeat for every file:
//!   u16 little-endian  length of the file name in bytes
//!   [u8]               file name, UTF-8, relative, "/" between folders (e.g. "prism.exe")
//!   u64 little-endian  file size in bytes
//!   [u8]               file contents
//! ```
//! A simple custom format keeps the setup free of extra archive libraries and lets us check
//! every name before anything is written (no absolute paths, no "..").

use flate2::{read::GzDecoder, write::GzEncoder, Compression};
use std::{
    fs,
    io::{self, Read, Write},
    path::Path,
};

/// Files bigger than this are rejected; a broken payload must not fill the disk.
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// One file inside the payload, as listed before extracting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PayloadFile {
    pub name: String,
    pub size: u64,
}

/// Builds a payload from `(name, contents)` pairs. Used by `build.rs` and tests.
// The setup program itself only reads payloads, so outside tests this is unused there.
#[cfg_attr(not(test), allow(dead_code))]
pub fn pack(files: &[(&str, &[u8])]) -> io::Result<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::best());
    for (name, contents) in files {
        check_name(name)?;
        encoder.write_all(&(name.len() as u16).to_le_bytes())?;
        encoder.write_all(name.as_bytes())?;
        encoder.write_all(&(contents.len() as u64).to_le_bytes())?;
        encoder.write_all(contents)?;
    }
    encoder.finish()
}

/// Rejects names that could write outside the install folder.
fn check_name(name: &str) -> io::Result<()> {
    crate::safe_path::relative_name(name)
}

fn read_exact_vec(reader: &mut impl Read, len: usize) -> io::Result<Vec<u8>> {
    let mut buffer = vec![0; len];
    reader.read_exact(&mut buffer)?;
    Ok(buffer)
}

/// Reads one file header. Returns `None` at the clean end of the payload.
fn read_header(reader: &mut impl Read) -> io::Result<Option<PayloadFile>> {
    let mut len = [0u8; 2];
    // A clean end of data before a header means "no more files".
    match reader.read(&mut len[..1])? {
        0 => return Ok(None),
        _ => reader.read_exact(&mut len[1..])?,
    }
    let name = String::from_utf8(read_exact_vec(reader, u16::from_le_bytes(len) as usize)?)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Bad file name in setup."))?;
    check_name(&name)?;
    let mut size = [0u8; 8];
    reader.read_exact(&mut size)?;
    let size = u64::from_le_bytes(size);
    if size > MAX_FILE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "The setup contains a file that is too large.",
        ));
    }
    Ok(Some(PayloadFile { name, size }))
}

/// Lists the files without writing anything (used to show the size and to validate the payload).
pub fn list(payload: &[u8]) -> io::Result<Vec<PayloadFile>> {
    let mut reader = GzDecoder::new(payload);
    let mut files = Vec::new();
    let mut names = std::collections::HashSet::new();
    let mut total = 0u64;
    while let Some(file) = read_header(&mut reader)? {
        total = total
            .checked_add(file.size)
            .ok_or_else(|| io::Error::other("Payload size overflow."))?;
        if files.len() >= 10_000
            || total > 8 * 1024 * 1024 * 1024
            || !names.insert(file.name.to_lowercase())
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Duplicate files or excessive payload size.",
            ));
        }
        if io::copy(&mut (&mut reader).take(file.size), &mut io::sink())? != file.size {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Incomplete payload file.",
            ));
        }
        files.push(file);
    }
    Ok(files)
}

/// Extracts every file into `dir`.
///
/// Each file is first written as `<name>.new` and then renamed over the old file, so a failure
/// half-way never leaves a broken `prism.exe` behind. `progress(done, total)` reports bytes.
/// Returns the relative names that were written (they go into the install record).
pub fn extract(
    payload: &[u8],
    dir: &Path,
    mut progress: impl FnMut(u64, u64),
) -> io::Result<Vec<String>> {
    let total: u64 = list(payload)?.iter().map(|file| file.size).sum();
    let dir = crate::safe_path::root(dir)?;
    let mut done = 0u64;
    let mut reader = GzDecoder::new(payload);
    let mut written = Vec::new();
    while let Some(file) = read_header(&mut reader)? {
        let target = crate::safe_path::create_parent(&dir, &file.name)?;
        let temporary = crate::safe_path::file(&dir, &format!("{}.new", file.name))?;
        {
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            let mut remaining = file.size;
            let mut buffer = vec![0u8; 256 * 1024];
            while remaining > 0 {
                let chunk = remaining.min(buffer.len() as u64) as usize;
                reader.read_exact(&mut buffer[..chunk])?;
                output.write_all(&buffer[..chunk])?;
                remaining -= chunk as u64;
                done += chunk as u64;
                progress(done, total);
            }
            output.sync_all()?;
        }
        crate::safe_path::file(&dir, &file.name)?;
        crate::safe_path::file(&dir, &format!("{}.new", file.name))?;
        replace_file(&temporary, &target)?;
        written.push(file.name);
    }
    progress(total, total);
    Ok(written)
}

/// Moves `from` over `to`. If `to` is busy (for example Prism is still closing after the
/// updater started us), it retries for a few seconds before giving up.
pub fn replace_file(from: &Path, to: &Path) -> io::Result<()> {
    let mut last_error = None;
    for _ in 0..40 {
        match fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(error) => last_error = Some(error),
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    let _ = fs::remove_file(from);
    Err(last_error.unwrap_or_else(|| io::Error::other("Could not replace a file.")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_files_are_listed_and_extracted_with_progress() {
        let big = vec![7u8; 600 * 1024];
        let payload = pack(&[("prism.exe", b"MZ fake app"), ("resources/data.bin", &big)]).unwrap();
        let files = list(&payload).unwrap();
        assert_eq!(
            files,
            [
                PayloadFile {
                    name: "prism.exe".into(),
                    size: 11
                },
                PayloadFile {
                    name: "resources/data.bin".into(),
                    size: big.len() as u64
                }
            ]
        );
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("prism.exe"), b"old version").unwrap();
        let mut reports = Vec::new();
        let written = extract(&payload, dir.path(), |done, total| {
            reports.push((done, total))
        })
        .unwrap();
        assert_eq!(written, ["prism.exe", "resources/data.bin"]);
        assert_eq!(
            fs::read(dir.path().join("prism.exe")).unwrap(),
            b"MZ fake app"
        );
        assert_eq!(
            fs::read(dir.path().join("resources/data.bin")).unwrap(),
            big
        );
        assert!(
            !dir.path().join("prism.exe.new").exists(),
            "temporary file removed"
        );
        let total = 11 + big.len() as u64;
        assert_eq!(reports.last(), Some(&(total, total)));
        assert!(
            reports.windows(2).all(|pair| pair[0].0 <= pair[1].0),
            "progress only grows"
        );
    }

    #[test]
    fn unsafe_names_and_broken_data_are_rejected() {
        for name in [
            "../evil.exe",
            "C:/evil.exe",
            "/abs.exe",
            r"dir\evil.exe",
            "",
            "a/../../b",
        ] {
            assert!(pack(&[(name, b"x")]).is_err(), "{name:?} must be rejected");
        }
        assert!(list(b"not gzip").is_err());
        let payload = pack(&[("prism.exe", b"0123456789")]).unwrap();
        let truncated = &payload[..payload.len() - 6];
        assert!(list(truncated).is_err());
        // An empty payload (development build without the app) is valid and has no files.
        assert!(list(&pack(&[]).unwrap()).unwrap().is_empty());
    }

    #[test]
    fn duplicates_and_short_bodies_are_rejected_before_extraction() {
        assert!(list(&pack(&[("prism.exe", b"a"), ("Prism.exe", b"b")]).unwrap()).is_err());
        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(&9u16.to_le_bytes()).unwrap();
        encoder.write_all(b"prism.exe").unwrap();
        encoder.write_all(&10u64.to_le_bytes()).unwrap();
        encoder.write_all(b"short").unwrap();
        let broken = encoder.finish().unwrap();
        assert!(list(&broken).is_err());
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("prism.exe"), b"old").unwrap();
        assert!(extract(&broken, dir.path(), |_, _| {}).is_err());
        assert_eq!(fs::read(dir.path().join("prism.exe")).unwrap(), b"old");
    }
}

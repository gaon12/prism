//! Temporary uninstall copies and a small cached native cleanup worker.
//! Only fixed files in a directly owned OS-temp folder are removed; no shell or recursion.
use crate::safe_path;
use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
const PREFIX: &str = "prism-uninstall-";
const MARKER: &str = "owner.txt";
const MAGIC: &[u8] = b"Prism temporary uninstall copy v1\n";

fn invalid() -> io::Error {
    io::Error::other("The temporary uninstall folder is not owned by this setup.")
}
fn temp_root() -> io::Result<PathBuf> {
    let root = fs::canonicalize(std::env::temp_dir())?;
    safe_path::no_links(&root)?;
    Ok(root)
}

pub fn copy_uninstaller(source: &Path) -> io::Result<PathBuf> {
    let root = temp_root()?;
    for _ in 0..128 {
        let directory = root.join(format!(
            "{PREFIX}{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        match fs::create_dir(&directory) {
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
            _ => {}
        }
        let copied = (|| {
            let destination = safe_path::file(&directory, "uninstall.exe")?;
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&destination)?;
            io::copy(&mut fs::File::open(source)?, &mut output)?;
            output.sync_all()?;
            let marker = safe_path::file(&directory, MARKER)?;
            let mut owner = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(marker)?;
            owner.write_all(MAGIC)?;
            owner.sync_all()?;
            Ok(destination)
        })();
        if copied.is_err() {
            // These exact newly created files are the only cleanup targets, even on failure.
            let _ = fs::remove_file(directory.join("uninstall.exe"));
            let _ = fs::remove_file(directory.join(MARKER));
            let _ = fs::remove_dir(&directory);
        }
        return copied;
    }
    Err(io::Error::other(
        "Could not create a private temporary uninstall folder.",
    ))
}

pub fn validate_directory(directory: &Path) -> io::Result<PathBuf> {
    let root = temp_root()?;
    let directory = safe_path::root(directory)?;
    if directory.parent() != Some(root.as_path()) {
        return Err(invalid());
    }
    let name = directory
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(invalid)?;
    let suffix = name.strip_prefix(PREFIX).ok_or_else(invalid)?;
    let parts: Vec<&str> = suffix.split('-').collect();
    if parts.len() != 2
        || parts
            .iter()
            .any(|part| part.is_empty() || !part.bytes().all(|value| value.is_ascii_digit()))
    {
        return Err(invalid());
    }
    let marker = safe_path::file(&directory, MARKER)?;
    let mut bytes = Vec::new();
    fs::File::open(marker)?.take(128).read_to_end(&mut bytes)?;
    if bytes != MAGIC {
        return Err(invalid());
    }
    safe_path::file(&directory, "uninstall.exe")?;
    Ok(directory)
}

pub fn own_directory(exe: &Path) -> io::Result<PathBuf> {
    let directory = validate_directory(exe.parent().ok_or_else(invalid)?)?;
    if !safe_path::same_target(exe, &directory.join("uninstall.exe")) {
        return Err(invalid());
    }
    Ok(directory)
}

pub fn remove_released_copy(directory: &Path) -> io::Result<()> {
    let directory = validate_directory(directory)?;
    let target = safe_path::file(&directory, "uninstall.exe")?;
    match fs::remove_file(target) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    fs::remove_file(safe_path::file(&directory, MARKER)?)?;
    // Unexpected files are preserved; never recursively remove a directory from arguments.
    fs::remove_dir(directory)
}

// Called by the separately compiled cleanup_main worker, not the setup binary.
#[cfg_attr(not(test), allow(dead_code))]
pub fn worker(directory: &Path) -> io::Result<()> {
    validate_directory(directory)?;
    for _ in 0..100 {
        match remove_released_copy(directory) {
            Ok(()) => return Ok(()),
            Err(error)
                if error.kind() == io::ErrorKind::PermissionDenied
                    || matches!(error.raw_os_error(), Some(32 | 33)) =>
            {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::other(
        "The temporary uninstaller did not exit in time.",
    ))
}

/// The worker remains as a small per-user OS-temp cache; its own executable is not a deletion target.
pub fn deploy_worker(bytes: &[u8]) -> io::Result<PathBuf> {
    if bytes.is_empty() || bytes.len() > 2 * 1024 * 1024 {
        return Err(invalid());
    }
    let directory = temp_root()?.join("PrismSetupTools");
    match fs::create_dir(&directory) {
        Err(error) if error.kind() != io::ErrorKind::AlreadyExists => return Err(error),
        _ => {}
    }
    let hash = bytes.iter().fold(0xcbf29ce484222325u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    });
    let destination = safe_path::file(&directory, &format!("prism-cleanup-{hash:016x}.exe"))?;
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&destination)
    {
        Ok(mut file) => {
            file.write_all(bytes)?;
            file.sync_all()?;
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    // Do not execute an existing file on the strength of its name or hash alone.
    if fs::metadata(&destination)?.len() != bytes.len() as u64 || fs::read(&destination)? != bytes {
        return Err(io::Error::other(
            "The cached cleanup worker does not match this setup.",
        ));
    }
    Ok(destination)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_owned_temporary_copies_are_removed() {
        let source = tempfile::NamedTempFile::new().unwrap();
        fs::write(source.path(), b"fixture, never executed").unwrap();
        let copy = copy_uninstaller(source.path()).unwrap();
        let directory = own_directory(&copy).unwrap();
        assert!(own_directory(source.path()).is_err());
        assert!(worker(source.path().parent().unwrap()).is_err());
        remove_released_copy(&directory).unwrap();
        assert!(!directory.exists());
        assert!(source.path().exists());
    }
    #[test]
    fn unexpected_files_are_preserved_without_recursive_removal() {
        let source = tempfile::NamedTempFile::new().unwrap();
        let copy = copy_uninstaller(source.path()).unwrap();
        let directory = own_directory(&copy).unwrap();
        fs::write(directory.join("personal.txt"), b"keep").unwrap();
        assert!(remove_released_copy(&directory).is_err());
        assert_eq!(fs::read(directory.join("personal.txt")).unwrap(), b"keep");
        // Explicitly clean this test's known file after checking that production preserved it.
        fs::remove_file(directory.join("personal.txt")).unwrap();
        fs::remove_dir(directory).unwrap();
    }
}

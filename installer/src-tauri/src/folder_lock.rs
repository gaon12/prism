//! An OS file lock serializes independent setup processes targeting the same folder.
//! The lock is outside the install folder so uninstall can remove an empty install directory.
use crate::safe_path;
use std::{fs, io, path::Path};

pub struct FolderLock {
    _file: fs::File,
}
impl FolderLock {
    pub fn acquire(folder: &Path) -> io::Result<Self> {
        let root = safe_path::root(folder)?;
        let mut key = root.to_string_lossy().into_owned();
        if cfg!(windows) {
            key = key.to_lowercase();
        }
        // Stable FNV-1a: a collision only serializes unrelated installs, never bypasses locking.
        let hash = key.bytes().fold(0xcbf29ce484222325u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
        });
        let temp = fs::canonicalize(std::env::temp_dir())?;
        safe_path::no_links(&temp)?;
        let directory = temp.join("PrismSetupLocks");
        match fs::create_dir(&directory) {
            Err(error) if error.kind() != io::ErrorKind::AlreadyExists => return Err(error),
            _ => {}
        }
        let path = safe_path::file(&directory, &format!("{hash:016x}.lock"))?;
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        file.try_lock()
            .map_err(|_| io::Error::other("Another setup is using this installation folder."))?;
        Ok(Self { _file: file })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn independent_handles_cannot_lock_the_same_folder_until_release() {
        let temp = tempfile::tempdir().unwrap();
        let first = FolderLock::acquire(temp.path()).unwrap();
        assert!(FolderLock::acquire(temp.path()).is_err());
        let other = tempfile::tempdir().unwrap();
        assert!(FolderLock::acquire(other.path()).is_ok());
        drop(first);
        assert!(FolderLock::acquire(temp.path()).is_ok());
    }
}

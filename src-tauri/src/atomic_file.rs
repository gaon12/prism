//! Replace one file with complete bytes; callers own encoding and parent setup.
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

struct Pending(Option<PathBuf>);

impl Drop for Pending {
    fn drop(&mut self) {
        if let Some(path) = &self.0 {
            let _ = fs::remove_file(path);
        }
    }
}

pub fn write(path: &Path, contents: &[u8]) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "File has no parent directory.")
    })?;
    let mut nonce = [0; 16];
    getrandom::fill(&mut nonce)
        .map_err(|_| io::Error::other("Cannot prepare file replacement."))?;
    // A fixed bounded name also preserves support for long destination names.
    let temporary = parent.join(format!(
        ".prism-write-{:032x}.tmp",
        u128::from_le_bytes(nonce)
    ));
    // Acquire exclusive ownership before arming cleanup. A collision must not
    // truncate or delete any pre-existing entry, including a linked one.
    // File drops before the guard on ordinary errors, including on Windows.
    let mut pending = Pending(None);
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    pending.0 = Some(temporary.clone());
    file.write_all(contents)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temporary, path)?;
    // Publication consumed the entry; a later file at this name is not ours.
    pending.0 = None;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacement_preserves_binary_bytes_and_the_previous_hardlink_peer() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("image.png");
        let peer = root.path().join("previous.png");
        fs::write(&path, b"previous\0bytes").unwrap();
        fs::hard_link(&path, &peer).unwrap();
        let contents = b"\x89PNG\r\n\x1a\n\0\xffbinary bytes";
        write(&path, contents).unwrap();
        assert_eq!(fs::read(&path).unwrap(), contents);
        assert_eq!(fs::read(&peer).unwrap(), b"previous\0bytes");
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 2);
    }

    #[test]
    fn stale_pending_hardlink_does_not_rewrite_an_unrelated_file() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("image.png");
        let original = root.path().join("unrelated.dat");
        fs::write(&original, b"preserve unrelated bytes").unwrap();
        let stale = path.with_extension("pending");
        fs::hard_link(&original, &stale).unwrap();
        write(&path, b"new\0image\xffbytes").unwrap();
        assert_eq!(fs::read(&original).unwrap(), b"preserve unrelated bytes");
        assert_eq!(fs::read(&stale).unwrap(), b"preserve unrelated bytes");
        assert_eq!(fs::read(&path).unwrap(), b"new\0image\xffbytes");
    }

    #[cfg(unix)]
    #[test]
    fn stale_pending_symlink_does_not_rewrite_an_unrelated_file() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("image.png");
        let original = root.path().join("unrelated.dat");
        fs::write(&original, b"preserve unrelated bytes").unwrap();
        let stale = path.with_extension("pending");
        std::os::unix::fs::symlink(&original, &stale).unwrap();
        write(&path, b"new image bytes").unwrap();
        assert_eq!(fs::read(&original).unwrap(), b"preserve unrelated bytes");
        assert!(fs::symlink_metadata(&stale).unwrap().is_symlink());
        assert_eq!(fs::read(&path).unwrap(), b"new image bytes");
    }

    #[test]
    fn failed_publication_preserves_destination_and_removes_owned_staging() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("blocked.json");
        fs::create_dir(&path).unwrap();
        fs::write(path.join("keep"), b"preserve original").unwrap();
        assert!(write(&path, b"new bytes").is_err());
        assert_eq!(fs::read(path.join("keep")).unwrap(), b"preserve original");
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }
}

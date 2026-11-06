//! Publish a completed export beside its selected destination. The caller owns
//! format validation and serializes cancellation against the final replacement.
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Input,
    Storage,
}
struct Pending(Option<PathBuf>);
impl Drop for Pending {
    fn drop(&mut self) {
        if let Some(path) = &self.0 {
            let _ = fs::remove_file(path);
        }
    }
}
fn stage(path: PathBuf) -> Result<(Pending, File), Error> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    // Acquire ownership before arming cleanup. A create_new collision must not
    // delete somebody else's existing entry, even though the name is random.
    let file = options.open(&path).map_err(|_| Error::Storage)?;
    Ok((Pending(Some(path)), file))
}
pub fn validate(path: &Path, extension: &str) -> Result<(), Error> {
    if extension.is_empty()
        || !extension.bytes().all(|b| b.is_ascii_alphanumeric())
        || !path.is_absolute()
        || path.as_os_str().as_encoded_bytes().contains(&0)
        || !path
            .extension()
            .is_some_and(|v| v.eq_ignore_ascii_case(extension))
        || path.file_name().is_none()
    {
        return Err(Error::Input);
    }
    Ok(())
}

/// `commit` must recheck cancellation and replace the staging entry with
/// `rename` under the operation's commit lock. It must not copy bytes onto an
/// existing destination: replacement preserves the contents of hardlink peers.
pub fn save<E: From<Error>>(
    path: &Path,
    extension: &str,
    bytes: &[u8],
    maximum: usize,
    check: impl Fn() -> Result<(), E>,
    commit: impl FnOnce(&Path, &Path) -> Result<(), E>,
) -> Result<(), E> {
    validate(path, extension)?;
    check()?;
    if bytes.is_empty() || bytes.len() > maximum {
        return Err(Error::Input.into());
    }
    let parent =
        fs::canonicalize(path.parent().ok_or(Error::Input)?).map_err(|_| Error::Storage)?;
    let destination = parent.join(path.file_name().ok_or(Error::Input)?);
    match fs::symlink_metadata(&destination) {
        Ok(meta) if !meta.is_file() || meta.file_type().is_symlink() => {
            return Err(Error::Input.into());
        }
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(Error::Storage.into()),
        _ => {}
    }
    let mut nonce = [0; 16];
    getrandom::fill(&mut nonce).map_err(|_| Error::Storage)?;
    let staging_path = parent.join(format!(
        ".prism-export-{:032x}.tmp",
        u128::from_le_bytes(nonce)
    ));
    let (mut pending, mut file) = stage(staging_path.clone())?;
    for chunk in bytes.chunks(64 << 10) {
        check()?;
        file.write_all(chunk).map_err(|_| Error::Storage)?;
    }
    check()?;
    file.sync_all().map_err(|_| Error::Storage)?;
    drop(file);
    commit(&staging_path, &destination)?;
    // Successful publication consumed our entry. Do not remove a subsequently
    // created file at the old staging name when the guard goes out of scope.
    pending.0 = None;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[derive(Debug, PartialEq, Eq)]
    enum Failure {
        File(Error),
        Cancelled,
    }
    impl From<Error> for Failure {
        fn from(value: Error) -> Self {
            Self::File(value)
        }
    }
    fn replace(stage: &Path, destination: &Path) -> Result<(), Failure> {
        fs::rename(stage, destination).map_err(|_| Error::Storage.into())
    }

    #[test]
    fn replacement_preserves_hardlink_peers_and_cancellation_preserves_destination() {
        let directory = tempfile::tempdir().unwrap();
        let original = directory.path().join("original.hwpx");
        let selected = directory.path().join("선택.HWPX");
        fs::write(&original, b"original data").unwrap();
        fs::hard_link(&original, &selected).unwrap();
        save(&selected, "hwpx", b"new export", 1024, || Ok(()), replace).unwrap();
        assert_eq!(fs::read(&selected).unwrap(), b"new export");
        assert_eq!(fs::read(&original).unwrap(), b"original data");

        // Let one block reach the staging file, then interrupt the next block.
        let checks = Cell::new(0);
        let result = save(
            &selected,
            "hwpx",
            &vec![b'x'; 128 << 10],
            128 << 10,
            || {
                checks.set(checks.get() + 1);
                if checks.get() >= 3 {
                    Err(Failure::Cancelled)
                } else {
                    Ok(())
                }
            },
            |_, _| panic!("An interrupted write must never publish"),
        );
        assert_eq!(result, Err(Failure::Cancelled));
        assert_eq!(fs::read(&selected).unwrap(), b"new export");
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2);

        let result = save(
            &selected,
            "hwpx",
            b"complete but cancelled before commit",
            1024,
            || Ok(()),
            |_, _| Err(Failure::Cancelled),
        );
        assert_eq!(result, Err(Failure::Cancelled));
        assert_eq!(fs::read(&selected).unwrap(), b"new export");
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2);
    }

    #[test]
    fn failed_stage_acquisition_never_deletes_existing_entry() {
        let directory = tempfile::tempdir().unwrap();
        let original = directory.path().join("original.hwpx");
        let collision = directory.path().join("reserved.tmp");
        fs::write(&original, b"keep original").unwrap();
        fs::hard_link(&original, &collision).unwrap();
        assert!(matches!(stage(collision.clone()), Err(Error::Storage)));
        assert_eq!(fs::read(&collision).unwrap(), b"keep original");
        assert_eq!(fs::read(&original).unwrap(), b"keep original");
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2);
    }

    #[test]
    fn invalid_destinations_and_payloads_do_not_create_staging_files() {
        let directory = tempfile::tempdir().unwrap();
        let folder = directory.path().join("folder.hwpx");
        fs::create_dir(&folder).unwrap();
        for path in [folder, directory.path().join("wrong.pdf")] {
            assert_eq!(
                save(&path, "hwpx", b"data", 8, || Ok(()), replace),
                Err(Failure::File(Error::Input))
            );
        }
        let path = directory.path().join("new.hwpx");
        for bytes in [b"".as_slice(), b"over limit"] {
            assert_eq!(
                save(&path, "hwpx", bytes, 8, || Ok(()), replace),
                Err(Failure::File(Error::Input))
            );
        }
        assert_eq!(
            validate(Path::new("relative.hwpx"), "hwpx"),
            Err(Error::Input)
        );
        assert_eq!(validate(&path, "../hwpx"), Err(Error::Input));
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn symbolic_link_is_rejected_and_private_staging_is_owner_only() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let directory = tempfile::tempdir().unwrap();
        let original = directory.path().join("original.hwpx");
        let selected = directory.path().join("link.hwpx");
        fs::write(&original, b"keep original").unwrap();
        symlink(&original, &selected).unwrap();
        assert_eq!(
            save(&selected, "hwpx", b"new export", 1024, || Ok(()), replace),
            Err(Failure::File(Error::Input))
        );
        let (guard, file) = stage(directory.path().join("owner.tmp")).unwrap();
        assert_eq!(file.metadata().unwrap().permissions().mode() & 0o777, 0o600);
        drop(file);
        drop(guard);
        assert_eq!(fs::read(&original).unwrap(), b"keep original");
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2);
    }
}

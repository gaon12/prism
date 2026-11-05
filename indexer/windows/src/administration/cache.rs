//! Private, fixed-location service metadata cache. An ownership marker reserves
//! drive slots and one staging file; no request chooses a filesystem path. The last valid cache
//! survives cancellation/crashes until a complete fsynced stage is renamed.
use super::*;
use prism_ntfs_core::{
    layout,
    snapshot::{codec, Snapshot},
};
use prism_ntfs_protocol::DriveLetter;
use std::io::{BufReader, BufWriter, Read, Seek};

const PRIVATE_DIRECTORY: &str = "O:BAG:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)";
const PRIVATE_FILE: &str = "O:BAG:BAD:P(A;;FA;;;SY)(A;;FA;;;BA)";
const LEGACY_MARKER: &[u8] =
    b"Prism filename metadata cache v1\ncatalog.pnc\ncatalog.next\ntransaction.lock\n";
const MARKER: &[u8] = b"Prism filename metadata cache v2\ncatalog-[A-Z].pnc\ncatalog.pnc (legacy)\ncatalog.next\ntransaction.lock\n";
const TOTAL_BUDGET: u64 = 2 * 1024 * 1024 * 1024;

pub(crate) struct Store {
    directory: PathBuf,
    drive: DriveLetter,
    _lock: File,
    _directory: OwnedHandle,
    _service: VerifiedService,
}

fn regular_single_link(file: &File) -> io::Result<()> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if info.dwFileAttributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT) != 0
        || info.nNumberOfLinks != 1
    {
        return Err(denied());
    }
    Ok(())
}
fn private_file(
    path: &Path,
    write: bool,
    disposition: u32,
    delete_on_close: bool,
) -> io::Result<File> {
    let descriptor = Descriptor::parse(PRIVATE_FILE)?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.raw(),
        bInheritHandle: 0,
    };
    let raw = unsafe {
        CreateFileW(
            wide(path).as_ptr(),
            GENERIC_READ | READ_SECURITY | if write { GENERIC_WRITE | DELETE } else { 0 },
            if write { 0 } else { FILE_SHARE_READ },
            &attributes,
            disposition,
            FILE_FLAG_OPEN_REPARSE_POINT
                | if delete_on_close {
                    FILE_FLAG_DELETE_ON_CLOSE
                } else {
                    0
                },
            null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let file = unsafe { File::from_raw_handle(raw) };
    regular_single_link(&file)?;
    protected_handle(&file, &descriptor)?;
    Ok(file)
}
fn existing_or_new(path: &Path) -> io::Result<File> {
    match private_file(path, true, CREATE_NEW, false) {
        Ok(file) => Ok(file),
        Err(error) if matches!(error.raw_os_error(), Some(80 | 183)) => {
            private_file(path, true, OPEN_EXISTING, false)
        }
        Err(error) => Err(error),
    }
}
fn set_delete(file: &File, delete: bool) -> io::Result<()> {
    let disposition = FILE_DISPOSITION_INFO { DeleteFile: delete };
    if unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileDispositionInfo,
            (&disposition as *const FILE_DISPOSITION_INFO).cast(),
            size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
fn write_new_marker(mut file: File) -> io::Result<()> {
    // The caller uses CREATE_NEW + FILE_FLAG_DELETE_ON_CLOSE, so even exit
    // immediately after creation removes our unfinished marker. Commit only
    // after the complete contents are durable. The extended ON_CLOSE operation
    // clears that flag; legacy FileDispositionInfo cannot cancel it. If the
    // filesystem does not support this operation, fail closed and let the
    // handle delete the new file rather than leave an unusable partial marker.
    file.write_all(MARKER)?;
    file.sync_all()?;
    let disposition = FILE_DISPOSITION_INFO_EX {
        Flags: FILE_DISPOSITION_FLAG_ON_CLOSE | FILE_DISPOSITION_FLAG_DO_NOT_DELETE,
    };
    if unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileDispositionInfoEx,
            (&disposition as *const FILE_DISPOSITION_INFO_EX).cast(),
            size_of::<FILE_DISPOSITION_INFO_EX>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
fn read_marker(file: &mut File) -> io::Result<bool> {
    let length = file.metadata()?.len();
    if length != MARKER.len() as u64 && length != LEGACY_MARKER.len() as u64 {
        return Err(denied());
    }
    let mut bytes = vec![0; length as usize];
    file.read_exact(&mut bytes)?;
    if bytes != MARKER && bytes != LEGACY_MARKER {
        return Err(denied());
    }
    Ok(bytes == LEGACY_MARKER)
}
fn transaction(path: &Path) -> io::Result<File> {
    match private_file(path, true, CREATE_NEW, true) {
        Ok(file) => Ok(file),
        Err(error) if matches!(error.raw_os_error(), Some(80 | 183)) => {
            // A live owner denies sharing. A stale lock may only be removed
            // after its exact private ACL, regular type and zero size verify.
            let file = private_file(path, true, OPEN_EXISTING, false)?;
            if file.metadata()?.len() != 0 {
                return Err(denied());
            }
            set_delete(&file, true)?;
            Ok(file)
        }
        Err(error) => Err(error),
    }
}
impl Store {
    pub(crate) fn open(drive: DriveLetter) -> io::Result<Self> {
        require_administrator()?;
        let service = verified_service(0)?;
        let directory = service.directory.join("cache-v1");
        let descriptor = Descriptor::parse(PRIVATE_DIRECTORY)?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.raw(),
            bInheritHandle: 0,
        };
        if unsafe { CreateDirectoryW(wide(&directory).as_ptr(), &attributes) } == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(ERROR_ALREADY_EXISTS as i32) {
                return Err(error);
            }
        }
        let guard = open_metadata(&directory, true)?;
        protected_handle(&guard, &descriptor)?;
        let marker = directory.join("format");
        let legacy = match private_file(&marker, false, OPEN_EXISTING, false) {
            Ok(mut file) => read_marker(&mut file)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                // Recover only an empty, already protected directory. Never
                // claim a nonempty unmarked directory or its existing files.
                if fs::read_dir(&directory)?.next().is_some() {
                    return Err(denied());
                }
                write_new_marker(private_file(&marker, true, CREATE_NEW, true)?)?;
                false
            }
            Err(error) => return Err(error),
        };
        let lock = transaction(&directory.join("transaction.lock"))?;
        let store = Self {
            directory,
            drive,
            _lock: lock,
            _directory: guard,
            _service: service,
        };
        if legacy {
            // The old marker already reserves catalog.next. Stage and fsync
            // the expanded ownership marker there before replacing format;
            // the old catalog remains available for a compatible first load.
            let stage = store.directory.join("catalog.next");
            let mut file = existing_or_new(&stage)?;
            file.set_len(0)?;
            file.rewind()?;
            file.write_all(MARKER)?;
            file.sync_all()?;
            drop(file);
            replace(&stage, &marker)?;
        }
        Ok(store)
    }
    fn target(&self) -> PathBuf {
        self.directory
            .join(format!("catalog-{}.pnc", self.drive.byte() as char))
    }
    pub(crate) fn load(
        &self,
        geometry: layout::Geometry,
        device: &str,
        check: impl Fn() -> layout::Result<()>,
    ) -> io::Result<Option<Snapshot>> {
        let file = match private_file(&self.target(), false, OPEN_EXISTING, false) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                match private_file(
                    &self.directory.join("catalog.pnc"),
                    false,
                    OPEN_EXISTING,
                    false,
                ) {
                    Ok(file) => file,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
                    Err(error) => return Err(error),
                }
            }
            Err(error) => return Err(error),
        };
        if file.metadata()?.len() > codec::MAX_BYTES + 48 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Metadata cache exceeds its bound.",
            ));
        }
        // This handle denies writes AND deletion throughout both digest and
        // decode passes. A metadata-only handle with WRITE sharing is unsafe.
        codec::read(
            &mut BufReader::with_capacity(1024 * 1024, file),
            geometry,
            device,
            check,
        )
        .map(Some)
    }
    pub(crate) fn save(
        &self,
        snapshot: &Snapshot,
        device: &str,
        check: impl Fn() -> layout::Result<()>,
    ) -> io::Result<()> {
        check().map_err(io::Error::other)?;
        let target = self.target();
        match private_file(&target, false, OPEN_EXISTING, false) {
            Ok(file) => drop(file),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let stage = self.directory.join("catalog.next");
        // The verified marker explicitly reserves this staging name. A prior
        // interrupted write can be truncated only AFTER actual-handle checks.
        let mut file = existing_or_new(&stage)?;
        file.set_len(0)?;
        file.rewind()?;
        {
            let mut writer = BufWriter::with_capacity(1024 * 1024, &mut file);
            codec::write(snapshot, device, &mut writer, &check)?;
            writer.flush()?;
        }
        file.sync_all()?;
        let size = file.metadata()?.len();
        drop(file);
        check().map_err(io::Error::other)?;
        self.make_room(size, &check)?;
        // Cache publication is one same-volume rename after data+checkpoint
        // validation and fsync. No copy-to-another-volume/reboot fallback.
        replace(&stage, &target)
    }
    fn make_room(&self, incoming: u64, check: &impl Fn() -> layout::Result<()>) -> io::Result<()> {
        let mut files = Vec::new();
        let mut total = incoming;
        // Only marker-reserved names are candidates. A single shared staging
        // file bounds interrupted writes too; unrelated children are ignored.
        for letter in (b'A'..=b'Z').map(Some).chain(std::iter::once(None)) {
            check().map_err(io::Error::other)?;
            if letter == Some(self.drive.byte()) {
                continue;
            }
            let name = letter.map_or_else(
                || "catalog.pnc".to_owned(),
                |letter| format!("catalog-{}.pnc", letter as char),
            );
            let file = match private_file(&self.directory.join(name), true, OPEN_EXISTING, false) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            let metadata = file.metadata()?;
            let length = metadata.len();
            total = total.saturating_add(length);
            files.push((
                metadata.modified().unwrap_or(std::time::UNIX_EPOCH),
                length,
                file,
            ));
        }
        files.sort_unstable_by_key(|(modified, _, _)| *modified);
        for (_, length, file) in files {
            if total <= TOTAL_BUDGET {
                break;
            }
            check().map_err(io::Error::other)?;
            set_delete(&file, true)?;
            total = total.saturating_sub(length);
        }
        if total > TOTAL_BUDGET {
            return Err(io::Error::new(
                io::ErrorKind::StorageFull,
                "Metadata cache budget exceeded.",
            ));
        }
        Ok(())
    }
}
fn remove_reserved(
    directory: &Path,
    legacy: bool,
    marker: &File,
    mut open: impl FnMut(&Path) -> io::Result<File>,
) -> io::Result<()> {
    let names = ["catalog.pnc".to_owned(), "catalog.next".to_owned()]
        .into_iter()
        .chain(
            (b'A'..=b'Z')
                .filter(move |_| !legacy)
                .map(|letter| format!("catalog-{}.pnc", letter as char)),
        );
    let mut files = Vec::new();
    // Validate and lock EVERY candidate before deleting any. A reserved name
    // alone never authorizes deleting an unexpected ACL/type/hard link. Older
    // markers do not reserve per-drive slots introduced by the new format.
    for name in names {
        match open(&directory.join(name)) {
            Ok(file) => files.push(file),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    for file in files {
        set_delete(&file, true)?;
    }
    // Keep the ownership proof until all payload deletions have succeeded, so
    // interruption can safely retry with whichever reserved files remain.
    set_delete(marker, true)
}
/// Called only after the exact verified service is stopped, before deleting
/// its SCM entry or image. There is no caller-selected path or recursive delete.
pub(super) fn remove_owned(service: &VerifiedService) -> io::Result<()> {
    let begin = Instant::now();
    loop {
        match remove_owned_once(service) {
            // SCM STOPPED can precede the old process closing its cache lock.
            // Give only native access/sharing locks a bounded grace period;
            // a failed ownership/type/marker check is never retried or ignored.
            Err(error)
                if matches!(error.raw_os_error(), Some(5 | 32 | 33))
                    && begin.elapsed() < Duration::from_secs(5) =>
            {
                std::thread::sleep(Duration::from_millis(20));
            }
            result => return result,
        }
    }
}
fn remove_owned_once(service: &VerifiedService) -> io::Result<()> {
    let directory = service.directory.join("cache-v1");
    let guard = match open_metadata(&directory, true) {
        Ok(guard) => guard,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    protected_handle(&guard, &Descriptor::parse(PRIVATE_DIRECTORY)?)?;
    let mut marker = match private_file(&directory.join("format"), true, OPEN_EXISTING, false) {
        Ok(marker) => marker,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            // An initial write interrupted before commit leaves an empty,
            // private directory. It owns no payloads; never claim nonempty
            // unmarked directories or recursively remove their children.
            if fs::read_dir(&directory)?.next().is_some() {
                return Err(denied());
            }
            drop(guard);
            return remove_empty_directory(&directory);
        }
        Err(error) => return Err(error),
    };
    let legacy = read_marker(&mut marker)?;
    // A writer still closing after SCM STOPPED makes this fail before deletion.
    // Never proceed by ignoring a live or unrecognized transaction lock.
    let lock = transaction(&directory.join("transaction.lock"))?;
    remove_reserved(&directory, legacy, &marker, |path| {
        private_file(path, true, OPEN_EXISTING, false)
    })?;
    drop(marker);
    drop(lock);
    drop(guard);
    remove_empty_directory(&directory)
}
fn remove_empty_directory(directory: &Path) -> io::Result<()> {
    match fs::remove_dir(directory) {
        Ok(()) => Ok(()),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::DirectoryNotEmpty
            ) =>
        {
            Ok(())
        }
        Err(error) => Err(error),
    }
}
fn replace(from: &Path, to: &Path) -> io::Result<()> {
    if unsafe {
        MoveFileExW(
            wide(from).as_ptr(),
            wide(to).as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::fs::OpenOptionsExt;

    // These handles belong to ordinary-user test fixtures. Production cleanup
    // always opens through private_file after verifying the machine directory
    // and marker. The callback lets native deletion semantics and the reserved
    // name policy be exercised without elevation or touching a real service.
    fn fixture_file(path: &Path) -> io::Result<File> {
        let file = fs::OpenOptions::new()
            .access_mode(GENERIC_READ | GENERIC_WRITE | DELETE)
            .share_mode(0)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?;
        regular_single_link(&file)?;
        Ok(file)
    }
    fn fixture_marker(path: &Path, write: bool) -> File {
        fs::OpenOptions::new()
            .write(true)
            .access_mode(GENERIC_READ | DELETE | if write { GENERIC_WRITE } else { 0 })
            .share_mode(0)
            .custom_flags(FILE_FLAG_DELETE_ON_CLOSE)
            .create_new(true)
            .open(path)
            .unwrap()
    }
    #[test]
    fn initial_marker_is_removed_before_commit_and_retained_after_fsync() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("format");
        drop(fixture_marker(&path, true));
        assert!(
            !path.exists(),
            "exit immediately after creation is recoverable"
        );
        let mut partial = fixture_marker(&path, true);
        partial.write_all(&MARKER[..8]).unwrap();
        drop(partial);
        assert!(!path.exists(), "an interrupted partial write is removed");
        write_new_marker(fixture_marker(&path, true)).unwrap();
        assert_eq!(fs::read(&path).unwrap(), MARKER);
    }
    #[test]
    fn failed_initial_marker_write_leaves_no_unmarked_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("format");
        assert!(write_new_marker(fixture_marker(&path, false)).is_err());
        assert!(!path.exists());
        write_new_marker(fixture_marker(&path, true)).unwrap();
        assert!(!read_marker(&mut File::open(&path).unwrap()).unwrap());
    }
    #[test]
    fn marker_crash_child() {
        let Some(directory) = std::env::var_os("PRISM_TEST_CACHE_CRASH_DIR") else {
            return;
        };
        let directory = PathBuf::from(directory);
        let mut file = fixture_marker(&directory.join("format"), true);
        if std::env::var("PRISM_TEST_CACHE_CRASH_PARTIAL").unwrap() == "true" {
            file.write_all(&MARKER[..8]).unwrap();
            file.sync_all().unwrap();
        }
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(directory.join("ready"))
            .unwrap();
        // Parent terminates this exact test-owned child, bypassing Rust Drop.
        // The bounded fallback prevents an orphan if the parent itself exits.
        std::thread::sleep(Duration::from_secs(30));
    }
    #[test]
    fn forced_process_exit_removes_new_and_partially_written_markers() {
        use std::os::windows::process::CommandExt;
        struct OwnedChild(std::process::Child);
        impl Drop for OwnedChild {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        for partial in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let mut child = OwnedChild(
                std::process::Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "administration::cache::tests::marker_crash_child",
                        "--nocapture",
                    ])
                    .env("PRISM_TEST_CACHE_CRASH_DIR", dir.path())
                    .env("PRISM_TEST_CACHE_CRASH_PARTIAL", partial.to_string())
                    .creation_flags(CREATE_NO_WINDOW)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                    .unwrap(),
            );
            let begin = Instant::now();
            while !dir.path().join("ready").exists() {
                assert!(
                    child.0.try_wait().unwrap().is_none(),
                    "owned child exited before readiness"
                );
                assert!(
                    begin.elapsed() < Duration::from_secs(10),
                    "owned child did not become ready"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
            child.0.kill().unwrap();
            child.0.wait().unwrap();
            assert!(
                !dir.path().join("format").exists(),
                "kernel must remove the uncommitted marker without Rust Drop"
            );
            // A new marker can be committed immediately after the crash.
            write_new_marker(fixture_marker(&dir.path().join("format"), true)).unwrap();
        }
    }
    #[test]
    fn cleanup_respects_each_marker_and_preserves_unknown_children() {
        for legacy in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let marker_path = dir.path().join("format");
            fs::write(&marker_path, if legacy { LEGACY_MARKER } else { MARKER }).unwrap();
            for name in ["catalog.pnc", "catalog.next", "catalog-C.pnc", "unrelated"] {
                fs::write(dir.path().join(name), b"owned fixture").unwrap();
            }
            let mut marker = fixture_file(&marker_path).unwrap();
            assert_eq!(read_marker(&mut marker).unwrap(), legacy);
            remove_reserved(dir.path(), legacy, &marker, fixture_file).unwrap();
            drop(marker);
            assert!(!marker_path.exists());
            assert!(!dir.path().join("catalog.pnc").exists());
            assert!(!dir.path().join("catalog.next").exists());
            assert_eq!(dir.path().join("catalog-C.pnc").exists(), legacy);
            assert_eq!(
                fs::read(dir.path().join("unrelated")).unwrap(),
                b"owned fixture"
            );
            remove_empty_directory(dir.path()).unwrap();
            assert!(dir.path().is_dir());
        }
    }
    #[test]
    fn invalid_reserved_file_stops_cleanup_before_any_deletion() {
        let dir = tempfile::tempdir().unwrap();
        let marker_path = dir.path().join("format");
        fs::write(&marker_path, MARKER).unwrap();
        fs::write(dir.path().join("catalog.pnc"), b"keep valid cache").unwrap();
        fs::write(dir.path().join("catalog-Z.pnc"), b"keep linked file").unwrap();
        fs::hard_link(
            dir.path().join("catalog-Z.pnc"),
            dir.path().join("unrelated"),
        )
        .unwrap();
        let marker = fixture_file(&marker_path).unwrap();
        assert_eq!(
            remove_reserved(dir.path(), false, &marker, fixture_file)
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
        drop(marker);
        assert_eq!(fs::read(marker_path).unwrap(), MARKER);
        assert_eq!(
            fs::read(dir.path().join("catalog.pnc")).unwrap(),
            b"keep valid cache"
        );
        assert_eq!(
            fs::read(dir.path().join("unrelated")).unwrap(),
            b"keep linked file"
        );
    }
    #[test]
    fn invalid_markers_never_authorize_reserved_cleanup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("format");
        for bytes in [b"".as_slice(), &MARKER[..8], b"not a Prism marker"] {
            fs::write(&path, bytes).unwrap();
            let mut file = fixture_file(&path).unwrap();
            assert_eq!(
                read_marker(&mut file).unwrap_err().kind(),
                io::ErrorKind::PermissionDenied
            );
            drop(file);
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }
    }
    #[test]
    fn ordinary_token_cannot_create_or_open_the_machine_cache() {
        if !is_elevated().unwrap() {
            assert!(
                matches!(Store::open(DriveLetter::new(b'C').unwrap()),Err(error) if error.kind()==io::ErrorKind::PermissionDenied)
            );
        }
    }
    #[test]
    fn regular_file_checks_reject_hard_links_and_preserve_their_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file");
        let link = dir.path().join("other");
        fs::write(&path, b"must remain").unwrap();
        let file = File::open(&path).unwrap();
        regular_single_link(&file).unwrap();
        fs::hard_link(&path, &link).unwrap();
        assert_eq!(
            regular_single_link(&file).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert_eq!(fs::read(&path).unwrap(), b"must remain");
    }
    #[test]
    fn ordinary_owned_files_are_not_accepted_as_private_service_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache");
        fs::write(&path, b"untrusted").unwrap();
        assert!(private_file(&path, false, OPEN_EXISTING, false).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"untrusted");
    }
}

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
            let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
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
            Ok(mut file) => {
                let length = file.metadata()?.len() as usize;
                if length != MARKER.len() && length != LEGACY_MARKER.len() {
                    return Err(denied());
                }
                let mut bytes = vec![0; length];
                file.read_exact(&mut bytes)?;
                if bytes != MARKER && bytes != LEGACY_MARKER {
                    return Err(denied());
                }
                bytes == LEGACY_MARKER
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                // Recover only an empty, already protected directory. Never
                // claim a nonempty unmarked directory or its existing files.
                if fs::read_dir(&directory)?.next().is_some() {
                    return Err(denied());
                }
                let mut file = private_file(&marker, true, CREATE_NEW, false)?;
                file.write_all(MARKER)?;
                file.sync_all()?;
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
            let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
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

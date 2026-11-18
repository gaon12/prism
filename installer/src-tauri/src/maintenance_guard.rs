//! Serialize maintenance of the per-user registration and shared application data.
//! A folder lock alone cannot exclude a setup working on another installation folder.

use crate::{install::System, safe_path};
use std::{fs, io, path::Path};

pub const FOREIGN_INSTALLATION: &str =
    "FOREIGN_INSTALLATION: This is not the registered Prism installation, or its Windows registration is unsafe. No shared registration or application data was changed.";
const BUSY: &str = "Another setup is changing Prism for this user. Wait for it to finish.";

/// Keeps the user's maintenance lock until the entire operation has finished.
pub struct Guard {
    _file: fs::File,
}

/// Acquire this before any installation-folder lock, recovery or shared mutation.
/// The system adapter supplies the base folder, so sandbox operations remain isolated.
pub fn acquire(system: &dyn System) -> Result<Guard, String> {
    let folders = system
        .folders()
        .map_err(|_| "Could not find the application folders.".to_string())?;
    let directory = folders.local_app_data.join("Prism Setup");
    ensure_directory(&directory)
        .map_err(|_| "The setup maintenance folder is unsafe or unavailable.".to_string())?;
    let path = safe_path::file(&directory, "maintenance.lock")
        .map_err(|_| "The setup maintenance lock path is unsafe.".to_string())?;
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // Keep the lock pathname from being removed/replaced while a guard owns it.
        options.share_mode(0x1 | 0x2); // FILE_SHARE_READ | FILE_SHARE_WRITE
    }
    let file = options
        .open(path)
        .map_err(|_| "Could not open the setup maintenance lock.".to_string())?;
    file.try_lock().map_err(|_| BUSY.to_string())?;
    Ok(Guard { _file: file })
}

// Never create a descendant through an existing junction, including when a sandbox's
// local-data folder does not exist yet. Recheck every newly created level as well.
fn ensure_directory(path: &Path) -> io::Result<()> {
    safe_path::no_links(path)?;
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => return Ok(()),
        Ok(_) => return Err(io::Error::other("Not a directory.")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("No maintenance parent folder."))?;
    ensure_directory(parent)?;
    safe_path::no_links(path)?;
    match fs::create_dir(path) {
        Err(error) if error.kind() != io::ErrorKind::AlreadyExists => return Err(error),
        _ => {}
    }
    safe_path::root(path).map(|_| ())
}

/// Recheck while holding `Guard`, immediately before touching shared data or registration.
/// An absent registration authorizes neither shared operation; foreign/invalid owners fail closed.
pub fn registered_owner(system: &dyn System, dir: &Path) -> Result<bool, String> {
    let Some((registered, _)) = system.existing_install() else {
        return Ok(false);
    };
    // Preserve the original spelling until Windows opens both directories. Case-folded
    // path comparisons can conflate distinct A/a folders on a case-sensitive volume.
    safe_path::no_links(dir).map_err(|_| FOREIGN_INSTALLATION.to_string())?;
    safe_path::no_links(&registered).map_err(|_| FOREIGN_INSTALLATION.to_string())?;
    if !same_directory(&registered, dir).map_err(|_| FOREIGN_INSTALLATION.to_string())? {
        return Err(FOREIGN_INSTALLATION.to_string());
    }
    Ok(true)
}

#[cfg(not(windows))]
fn same_directory(a: &Path, b: &Path) -> io::Result<bool> {
    Ok(safe_path::root(a)? == safe_path::root(b)?)
}

#[cfg(windows)]
fn same_directory(a: &Path, b: &Path) -> io::Result<bool> {
    let a = native_directory::Directory::open(a)?;
    let b = native_directory::Directory::open(b)?;
    Ok(a.identity == b.identity)
}

#[cfg(windows)]
mod native_directory {
    use super::*;
    use std::os::windows::ffi::OsStrExt;
    use windows::{
        core::PCWSTR,
        Win32::{
            Foundation::{CloseHandle, HANDLE},
            Storage::FileSystem::{
                CreateFileW, FileIdInfo, GetFileInformationByHandle, GetFileInformationByHandleEx,
                BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
                FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
                FILE_FLAG_POSIX_SEMANTICS, FILE_ID_INFO, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE,
                FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
            },
        },
    };

    struct Handle(HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: this wrapper owns one successful non-pseudo CreateFileW handle.
            let _ = unsafe { CloseHandle(self.0) };
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(super) struct Identity {
        volume: u64,
        file: [u8; 16],
    }

    pub(super) struct Directory {
        _handle: Handle,
        pub(super) identity: Identity,
    }
    impl Directory {
        pub(super) fn open(original: &Path) -> io::Result<Self> {
            safe_path::no_links(original)?;
            if original.parent().is_none() {
                return Err(io::Error::other(
                    "A drive root is not an installation directory.",
                ));
            }
            let mut name: Vec<u16> = original.as_os_str().encode_wide().collect();
            if name.contains(&0) {
                return Err(io::Error::other("Invalid directory spelling."));
            }
            for ch in &mut name {
                if *ch == b'/' as u16 {
                    *ch = b'\\' as u16;
                }
            }
            let prefix: Vec<u16> = r"\\?\".encode_utf16().collect();
            if !name.starts_with(&prefix) {
                name = if name.starts_with(&[b'\\' as u16, b'\\' as u16]) {
                    [
                        r"\\?\UNC\".encode_utf16().collect::<Vec<_>>(),
                        name[2..].to_vec(),
                    ]
                    .concat()
                } else {
                    [prefix, name].concat()
                };
            }
            name.push(0);
            // SAFETY: NUL-terminated original path; directory metadata access only.
            // POSIX semantics preserves case-sensitive spelling and OPEN_REPARSE_POINT
            // lets the metadata check refuse a final junction planted after no_links.
            let handle = Handle(
                unsafe {
                    CreateFileW(
                        PCWSTR(name.as_ptr()),
                        FILE_READ_ATTRIBUTES.0,
                        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                        None,
                        OPEN_EXISTING,
                        FILE_FLAG_BACKUP_SEMANTICS
                            | FILE_FLAG_POSIX_SEMANTICS
                            | FILE_FLAG_OPEN_REPARSE_POINT,
                        None,
                    )
                }
                .map_err(|error| io::Error::other(format!("Open directory identity: {error}")))?,
            );
            let mut attributes = BY_HANDLE_FILE_INFORMATION::default();
            // SAFETY: a retained directory handle and writable metadata structure.
            unsafe { GetFileInformationByHandle(handle.0, &mut attributes) }
                .map_err(|error| io::Error::other(format!("Read directory attributes: {error}")))?;
            if attributes.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0 == 0
                || attributes.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
            {
                return Err(io::Error::other("Identity is not a plain directory."));
            }
            let mut info = FILE_ID_INFO::default();
            // SAFETY: the buffer has the size/alignment required for FileIdInfo.
            unsafe {
                GetFileInformationByHandleEx(
                    handle.0,
                    FileIdInfo,
                    (&mut info as *mut FILE_ID_INFO).cast(),
                    std::mem::size_of::<FILE_ID_INFO>() as u32,
                )
            }
            .map_err(|error| io::Error::other(format!("Read directory identity: {error}")))?;
            if info.FileId.Identifier == [0; 16] {
                return Err(io::Error::other(
                    "Filesystem did not provide a directory identity.",
                ));
            }
            Ok(Self {
                _handle: handle,
                identity: Identity {
                    volume: info.VolumeSerialNumber,
                    file: info.FileId.Identifier,
                },
            })
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn volume_and_all_file_id_bits_distinguish_case_alias_identities() {
            let intended = Identity {
                volume: 7,
                file: [1; 16],
            };
            let mut other_case = intended;
            other_case.file[15] = 2;
            assert_ne!(intended, other_case);
            assert_ne!(
                intended,
                Identity {
                    volume: 8,
                    ..intended
                }
            );
        }

        #[test]
        fn native_directory_identity_is_stable_for_the_same_directory_and_distinct_for_siblings() {
            let temp = tempfile::tempdir().unwrap();
            let a = temp.path().join("directory A");
            let b = temp.path().join("directory B");
            fs::create_dir(&a).unwrap();
            fs::create_dir(&b).unwrap();
            let first = Directory::open(&a).unwrap();
            assert_eq!(first.identity, Directory::open(&a).unwrap().identity);
            assert_ne!(first.identity, Directory::open(&b).unwrap().identity);
            assert!(!same_directory(&a, &b).unwrap());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::{tests::FakeSystem, UninstallEntry};

    fn register(system: &FakeSystem, path: &Path) {
        *system.entry.borrow_mut() = Some(UninstallEntry {
            version: "0.1.0".into(),
            install_dir: path.into(),
            size_kib: 1,
        });
    }

    #[test]
    fn only_the_exact_registered_folder_authorizes_shared_mutations() {
        let temp = tempfile::tempdir().unwrap();
        let system = FakeSystem::new(temp.path());
        let a = temp.path().join("installation A");
        let b = temp.path().join("installation B");
        fs::create_dir(&a).unwrap();
        fs::create_dir(&b).unwrap();
        assert_eq!(registered_owner(&system, &a), Ok(false));
        register(&system, &a);
        assert_eq!(registered_owner(&system, &a), Ok(true));
        assert_eq!(
            registered_owner(&system, &b),
            Err(FOREIGN_INSTALLATION.into())
        );
        register(&system, &b);
        let before = system.capture_registration().unwrap();
        assert_eq!(
            registered_owner(&system, &a),
            Err(FOREIGN_INSTALLATION.into())
        );
        assert_eq!(system.capture_registration().unwrap(), before);
        assert_eq!(registered_owner(&system, &b), Ok(true));
    }

    #[test]
    fn invalid_or_missing_registered_folders_are_never_owners() {
        let temp = tempfile::tempdir().unwrap();
        let system = FakeSystem::new(temp.path());
        let app = temp.path().join("app");
        fs::create_dir(&app).unwrap();
        let file = temp.path().join("not-a-directory");
        fs::write(&file, b"untouched").unwrap();
        for path in [
            Path::new("relative").to_path_buf(),
            temp.path().join("missing"),
            temp.path().join("app/../app"),
            file,
        ] {
            register(&system, &path);
            assert_eq!(
                registered_owner(&system, &app),
                Err(FOREIGN_INSTALLATION.into())
            );
        }
    }

    #[test]
    fn one_user_cannot_maintain_two_targets_at_the_same_time() {
        let temp = tempfile::tempdir().unwrap();
        let first = FakeSystem::new(temp.path());
        let second = FakeSystem::new(temp.path());
        register(&first, &temp.path().join("A"));
        register(&second, &temp.path().join("B"));
        let lock = acquire(&first).unwrap();
        assert_eq!(acquire(&second).err().as_deref(), Some(BUSY));
        #[cfg(windows)]
        assert!(fs::remove_file(temp.path().join("Local/Prism Setup/maintenance.lock")).is_err());
        let other_user = tempfile::tempdir().unwrap();
        assert!(acquire(&FakeSystem::new(other_user.path())).is_ok());
        drop(lock);
        assert!(acquire(&second).is_ok());
        assert!(temp
            .path()
            .join("Local/Prism Setup/maintenance.lock")
            .is_file());
    }

    #[test]
    fn a_file_cannot_be_adopted_as_the_maintenance_folder() {
        let temp = tempfile::tempdir().unwrap();
        let system = FakeSystem::new(temp.path());
        fs::create_dir(temp.path().join("Local")).unwrap();
        let file = temp.path().join("Local/Prism Setup");
        fs::write(&file, b"personal file").unwrap();
        assert!(acquire(&system).is_err());
        assert_eq!(fs::read(file).unwrap(), b"personal file");
    }

    #[test]
    fn linked_maintenance_and_registered_folders_fail_without_external_writes() {
        let temp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let system = FakeSystem::new(temp.path());
        let link = temp.path().join("Local");
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path(), &link).unwrap();
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let output = std::process::Command::new("powershell.exe")
                .args(["-NoProfile", "-NonInteractive", "-Command", "New-Item -ItemType Junction -Path $env:PRISM_GUARD_TEST_LINK -Target $env:PRISM_GUARD_TEST_TARGET -ErrorAction Stop | Out-Null"])
                .env("PRISM_GUARD_TEST_LINK", &link)
                .env("PRISM_GUARD_TEST_TARGET", outside.path())
                .creation_flags(0x0800_0000)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        assert!(acquire(&system).is_err());
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
        register(&system, &link);
        assert_eq!(
            registered_owner(&system, outside.path()),
            Err(FOREIGN_INSTALLATION.into())
        );
    }
}

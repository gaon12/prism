//! Ordinary-user entry point: bind a registered drive root to an actual NTFS
//! root identity, authenticate SCM/pipe identity, and validate the full stream.
use crate::pipe::Connection;
use prism_ntfs_protocol::{stream::Event, DriveLetter, Request};
use std::{
    io,
    mem::size_of,
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::{Component, Path, Prefix},
    ptr::{null, null_mut},
    sync::{atomic::AtomicBool, Arc},
};
use windows_sys::Win32::{Foundation::*, Storage::FileSystem::*};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RootIdentity {
    pub drive: DriveLetter,
    pub serial: u64,
    pub reference: u64,
    pub device: String,
}
fn unsupported() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "The filename service requires a local NTFS volume root.",
    )
}

pub fn root_identity(path: &Path) -> io::Result<RootIdentity> {
    let mut components = path.components();
    let drive = match components.next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => DriveLetter::new(letter)?,
            _ => return Err(unsupported()),
        },
        _ => return Err(unsupported()),
    };
    if components.next() != Some(Component::RootDir) || components.next().is_some() {
        return Err(unsupported());
    }
    let wide: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: query the already-authorized root only. No volume read access,
    // inheritance, content read, following of a user-created link, or write.
    let raw = unsafe {
        CreateFileW(
            wide.as_ptr(),
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut identity = FILE_ID_INFO::default();
    // SAFETY: initialized numeric output and a valid directory metadata handle.
    if unsafe {
        GetFileInformationByHandleEx(
            handle.as_raw_handle(),
            FileIdInfo,
            (&mut identity as *mut FILE_ID_INFO).cast(),
            size_of::<FILE_ID_INFO>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let reference = u64::from_le_bytes(identity.FileId.Identifier[..8].try_into().unwrap());
    // A SUBST mapping into a subdirectory can have the same volume serial as
    // the service's C:, so serial alone is insufficient. NTFS root record 5
    // and a 64-bit NTFS reference are both required before accepting C: paths.
    if reference & 0x0000_ffff_ffff_ffff != 5
        || identity.FileId.Identifier[8..]
            .iter()
            .any(|byte| *byte != 0)
    {
        return Err(unsupported());
    }
    let mut final_path = vec![0u16; 1024];
    let length = unsafe {
        GetFinalPathNameByHandleW(
            handle.as_raw_handle(),
            final_path.as_mut_ptr(),
            final_path.len() as u32,
            VOLUME_NAME_NT,
        )
    } as usize;
    if length == 0 {
        return Err(io::Error::last_os_error());
    }
    if length >= final_path.len() {
        return Err(unsupported());
    }
    let device = String::from_utf16(&final_path[..length])
        .map_err(|_| unsupported())?
        .trim_end_matches('\\')
        .to_owned();
    if !prism_ntfs_protocol::valid_device_name(&device) {
        return Err(unsupported());
    }
    Ok(RootIdentity {
        drive,
        serial: identity.VolumeSerialNumber,
        reference,
        device,
    })
}
pub fn snapshot(
    root: &Path,
    cancel: Arc<AtomicBool>,
    receive: impl FnMut(Event) -> io::Result<()>,
) -> io::Result<u64> {
    snapshot_mode(root, cancel, receive, false)
}

/// Explicit metadata mode. A legacy service's rejection is Unsupported, not
/// permission to substitute rows that lack the requested attributes.
/// Individual rows may still report None when complete flags are unavailable.
pub fn snapshot_with_attributes(
    root: &Path,
    cancel: Arc<AtomicBool>,
    receive: impl FnMut(Event) -> io::Result<()>,
) -> io::Result<u64> {
    snapshot_mode(root, cancel, receive, true).map_err(attribute_error)
}

pub(crate) fn attribute_error(error: io::Error) -> io::Error {
    if error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<prism_ntfs_protocol::stream::Error>())
        == Some(&prism_ntfs_protocol::stream::Error::Service(
            prism_ntfs_protocol::FailureCode::InvalidRequest,
        ))
    {
        io::Error::new(io::ErrorKind::Unsupported, "The filename service does not support file attributes. Use the bundled service version or the filesystem walker.")
    } else {
        error
    }
}

fn snapshot_mode(
    root: &Path,
    cancel: Arc<AtomicBool>,
    receive: impl FnMut(Event) -> io::Result<()>,
    attributes: bool,
) -> io::Result<u64> {
    let identity = root_identity(root)?;
    let mut connection = Connection::connect(cancel)?;
    let revision = connection.exchange(
        if attributes {
            Request::SnapshotWithAttributes {
                drive: identity.drive,
            }
        } else {
            Request::Snapshot {
                drive: identity.drive,
            }
        },
        identity.serial,
        &identity.device,
        receive,
    )?;
    if root_identity(root)? != identity {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "The registered volume changed while collecting filenames.",
        ));
    }
    Ok(revision)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_an_explicit_unsupported_request_maps_to_attribute_unavailability() {
        use prism_ntfs_protocol::{stream::Error, FailureCode};
        let missing = attribute_error(io::Error::other(Error::Service(
            FailureCode::InvalidRequest,
        )));
        assert_eq!(missing.kind(), io::ErrorKind::Unsupported);
        for code in [
            FailureCode::AccessDenied,
            FailureCode::Unavailable,
            FailureCode::Cancelled,
            FailureCode::JournalGap,
        ] {
            let failure = Error::Service(code);
            let error = attribute_error(io::Error::other(failure));
            assert_eq!(error.kind(), io::ErrorKind::Other);
            assert_eq!(
                error.get_ref().unwrap().downcast_ref::<Error>(),
                Some(&failure)
            );
        }
        let error = attribute_error(io::Error::new(
            io::ErrorKind::InvalidData,
            Error::VolumeMismatch,
        ));
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(
            attribute_error(io::ErrorKind::Interrupted.into()).kind(),
            io::ErrorKind::Interrupted
        );
    }
    #[test]
    fn reject_relative_unc_and_subdirectory_inputs_before_opening() {
        for value in [
            "relative",
            r"\\server\share\",
            r"C:\Windows",
            r"C:\folder\..",
            r"\\.\C:",
        ] {
            assert_eq!(
                root_identity(Path::new(value)).unwrap_err().kind(),
                io::ErrorKind::Unsupported
            );
        }
    }
    #[test]
    fn ordinary_privileges_can_identify_the_actual_windows_system_root() {
        let root = std::env::var("SystemDrive").expect("Windows system drive") + r"\";
        let identity = root_identity(Path::new(&root)).unwrap();
        assert_eq!(identity.reference & 0x0000_ffff_ffff_ffff, 5);
        assert_eq!(identity, root_identity(Path::new(&root)).unwrap());
    }
}

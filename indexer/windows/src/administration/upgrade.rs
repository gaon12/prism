//! Explicit replacement of an already verified, protected service image.
//! The old image remains linked until the replacement has started successfully.
//! A leftover stage/backup is never silently consumed after an interrupted run.
use super::*;

fn create_protected(path: &Path, flags: u32) -> io::Result<File> {
    let descriptor = Descriptor::parse(FILE_ACL)?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.raw(),
        bInheritHandle: 0,
    };
    let raw = unsafe {
        CreateFileW(
            wide(path).as_ptr(),
            GENERIC_READ | GENERIC_WRITE | DELETE | READ_SECURITY,
            0,
            &attributes,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL | flags,
            null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_handle(raw) })
}
fn ensure_absent(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "An earlier upgrade artifact requires explicit recovery.",
        )),
    }
}
fn same_file(left: &Path, right: &Path) -> io::Result<bool> {
    let left = file_identity(&open_metadata(left, false)?)?;
    let right = file_identity(&open_metadata(right, false)?)?;
    Ok(left.VolumeSerialNumber == right.VolumeSerialNumber
        && left.FileId.Identifier == right.FileId.Identifier)
}
fn replace(from: &Path, to: &Path) -> io::Result<()> {
    let start = Instant::now();
    loop {
        // Same-directory rename only: never enable COPY_ALLOWED, defer until
        // reboot, merge streams or relax an ACL. The new image has FILE_ACL.
        if unsafe {
            MoveFileExW(
                wide(from).as_ptr(),
                wide(to).as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } != 0
        {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if !matches!(error.raw_os_error(), Some(5 | 32 | 33))
            || start.elapsed() >= Duration::from_secs(5)
        {
            return Err(error);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
fn start_verified(service: &ServiceHandle) -> io::Result<()> {
    if unsafe { StartServiceW(service.raw(), 0, null()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    wait_state(service, SERVICE_RUNNING)
}

/// Replace only the exact existing protected LocalSystem service with the
/// helper's compiled payload. No caller-provided source/destination is accepted.
/// Ordinary app startup never invokes this operation.
pub fn upgrade(payload: &[u8]) -> io::Result<()> {
    require_administrator()?;
    validate_payload(payload)?;
    let verified = verified_service(SERVICE_STOP | SERVICE_START)?;
    // Serializes distinct Prism copies/helpers too. DELETE_ON_CLOSE releases
    // this newly created lock on both normal exit and process termination.
    let _lock = create_protected(
        &verified.directory.join("prism-file-index.upgrade.lock"),
        FILE_FLAG_DELETE_ON_CLOSE,
    )?;
    let stage = verified.directory.join("prism-file-index.next.exe");
    let backup = verified.directory.join("prism-file-index.previous.exe");
    ensure_absent(&backup)?;
    let mut destination = create_protected(&stage, 0)?;
    let written = destination
        .write_all(payload)
        .and_then(|_| destination.sync_all());
    drop(destination);
    if let Err(error) = written {
        let _ = fs::remove_file(&stage); // Only our CREATE_NEW stage.
        return Err(error);
    }
    let result = (|| {
        let descriptor = Descriptor::parse(FILE_ACL)?;
        protected_handle(&open_metadata(&stage, false)?, &descriptor)?;
        // Hard-link preserves the original bytes/identity without copying a
        // running image. It fails if ANY file already occupies the backup name.
        if unsafe {
            CreateHardLinkW(
                wide(&backup).as_ptr(),
                wide(&verified.target).as_ptr(),
                null(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        if !same_file(&backup, &verified.target)? {
            return Err(denied());
        }
        let previous = service_state(&verified.service)?;
        let was_running = previous != SERVICE_STOPPED;
        let attempt = (|| {
            stop(&verified.service)?;
            replace(&stage, &verified.target)?;
            protected_handle(&open_metadata(&verified.target, false)?, &descriptor)?;
            if was_running {
                start_verified(&verified.service)?;
            }
            Ok(())
        })();
        if let Err(error) = attempt {
            // If replacement never happened, retain the old path and restart
            // it. Otherwise stop only our service, restore the linked original
            // and restart it. Never kill unrelated processes or delete the
            // backup when recovery fails; the old image remains available.
            let recovered = (|| {
                if !same_file(&backup, &verified.target)? {
                    stop(&verified.service)?;
                    protected_handle(&open_metadata(&backup, false)?, &descriptor)?;
                    replace(&backup, &verified.target)?;
                } else {
                    fs::remove_file(&backup)?;
                }
                if was_running && service_state(&verified.service)? != SERVICE_RUNNING {
                    start_verified(&verified.service)?;
                }
                Ok::<_, io::Error>(())
            })();
            return match recovered {
                Ok(()) => Err(error),
                Err(_) => Err(io::Error::other(
                    "Service upgrade recovery requires attention; the original image is preserved.",
                )),
            };
        }
        fs::remove_file(&backup)?;
        Ok(())
    })();
    // Successful rename consumes the stage. On ordinary failure it is still
    // our file; an interrupted transaction is retained and blocks a later run.
    let _ = fs::remove_file(&stage);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ordinary_token_cannot_upgrade_even_an_invalid_payload() {
        if !is_elevated().unwrap() {
            assert_eq!(
                upgrade(&[]).unwrap_err().kind(),
                io::ErrorKind::PermissionDenied
            );
        }
    }
    #[test]
    fn linked_original_survives_replacement_and_can_restore_the_old_image() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("host.exe");
        let stage = dir.path().join("next.exe");
        let backup = dir.path().join("previous.exe");
        fs::write(&target, b"original").unwrap();
        fs::write(&stage, b"replacement").unwrap();
        assert_ne!(
            unsafe { CreateHardLinkW(wide(&backup).as_ptr(), wide(&target).as_ptr(), null()) },
            0
        );
        assert!(same_file(&backup, &target).unwrap());
        replace(&stage, &target).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"replacement");
        assert_eq!(fs::read(&backup).unwrap(), b"original");
        assert!(!same_file(&backup, &target).unwrap());
        replace(&backup, &target).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"original");
        assert!(!backup.exists());
    }
    #[test]
    fn interrupted_artifacts_and_failed_replacements_preserve_existing_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("host.exe");
        fs::write(&target, b"existing").unwrap();
        assert_eq!(
            ensure_absent(&target).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert!(replace(&dir.path().join("missing.exe"), &target).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"existing");
    }
}

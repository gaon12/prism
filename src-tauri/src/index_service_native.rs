use super::{Action, Error, Issue, OperationStage};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    mem::size_of,
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        fs::{MetadataExt, OpenOptionsExt},
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::{Component, Path, PathBuf, Prefix},
};
use windows_sys::Win32::{
    Foundation::*,
    Security::{
        Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW, SECURITY_ATTRIBUTES,
    },
    Storage::FileSystem::*,
    System::{Com::*, Threading::*},
    UI::{Shell::*, WindowsAndMessaging::SW_HIDE},
};

const HELPER: &[u8] = include_bytes!(env!("PRISM_INDEX_ADMIN_HELPER"));
fn diagnostic(stage: OperationStage, native_code: Option<i32>) -> Error {
    Error {
        stage: Some(stage),
        native_code,
        ..Issue::Unavailable.into()
    }
}
fn unavailable(error: std::io::Error) -> Error {
    diagnostic(OperationStage::Staging, error.raw_os_error())
}
fn launch_failure(error: std::io::Error) -> Error {
    let mut result = diagnostic(OperationStage::Launch, error.raw_os_error());
    result.code = match error.raw_os_error() {
        Some(code) if code == ERROR_CANCELLED as i32 => Issue::Cancelled,
        Some(code) if code == ERROR_ACCESS_DENIED as i32 => Issue::PermissionDenied,
        _ => Issue::Unavailable,
    };
    result
}
fn wide(value: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
    value.as_ref().encode_wide().chain(Some(0)).collect()
}
fn shell_path(path: &Path) -> Result<PathBuf, Error> {
    let mut units: Vec<_> = path.as_os_str().encode_wide().collect();
    for component in path.components() {
        if let Component::Normal(value) = component {
            let value = value.to_string_lossy();
            if value.ends_with(['.', ' ']) {
                return Err(Issue::Unavailable.into());
            }
        }
    }
    match path.components().next() {
        Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::VerbatimDisk(_)) => {
            units.drain(..4);
        }
        Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::Disk(_)) => {}
        _ => return Err(Issue::Unavailable.into()),
    }
    if units.len() >= 260 {
        return Err(diagnostic(OperationStage::Staging, None));
    }
    Ok(PathBuf::from(std::ffi::OsString::from_wide(&units)))
}

struct StagedHelper {
    directory: PathBuf,
    path: PathBuf,
    image: Option<File>,
    directory_lock: Option<File>,
    _ancestors: Vec<File>,
    owns_file: bool,
}
fn lock_directory(path: &Path, writable_share: bool) -> std::io::Result<File> {
    let file = OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | if writable_share { FILE_SHARE_WRITE } else { 0 })
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "Unexpected helper directory.",
        ));
    }
    Ok(file)
}
fn file_id(file: &File) -> std::io::Result<(u64, [u8; 16])> {
    let mut identity = FILE_ID_INFO::default();
    if unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileIdInfo,
            (&mut identity as *mut FILE_ID_INFO).cast(),
            size_of::<FILE_ID_INFO>() as u32,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error());
    }
    Ok((identity.VolumeSerialNumber, identity.FileId.Identifier))
}
fn create_private_directory(path: &Path) -> Result<(), Error> {
    // A shared/custom TEMP must not let another ordinary account add a DLL
    // beside the elevated image. Owner Rights applies to the creating user;
    // administrators/System can also read it during credential-based UAC.
    let sddl = wide("D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FA;;;OW)");
    let mut descriptor = std::ptr::null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            1,
            &mut descriptor,
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(unavailable(std::io::Error::last_os_error()));
    }
    struct Descriptor(*mut std::ffi::c_void);
    impl Drop for Descriptor {
        fn drop(&mut self) {
            unsafe {
                LocalFree(self.0);
            }
        }
    }
    let descriptor = Descriptor(descriptor);
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    };
    if unsafe { CreateDirectoryW(wide(path).as_ptr(), &attributes) } == 0 {
        return Err(unavailable(std::io::Error::last_os_error()));
    }
    Ok(())
}
impl StagedHelper {
    fn create() -> Result<Self, Error> {
        let root = fs::canonicalize(std::env::temp_dir()).map_err(unavailable)?;
        let mut ancestors = Vec::new();
        let mut path = PathBuf::new();
        for component in root.components() {
            match component {
                Component::Prefix(prefix)
                    if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)) =>
                {
                    path.push(prefix.as_os_str());
                    continue;
                }
                Component::RootDir | Component::Normal(_) => path.push(component.as_os_str()),
                _ => return Err(Issue::Unavailable.into()),
            }
            ancestors.push(lock_directory(&path, true).map_err(unavailable)?);
        }
        let mut nonce = [0; 16];
        getrandom::fill(&mut nonce).map_err(|_| diagnostic(OperationStage::Staging, None))?;
        let directory = root.join(format!(
            "Prism-index-helper-{:032x}",
            u128::from_le_bytes(nonce)
        ));
        create_private_directory(&directory)?;
        let mut staged = Self {
            path: directory.join("prism-file-index-admin.exe"),
            directory,
            image: None,
            directory_lock: None,
            _ancestors: ancestors,
            owns_file: false,
        };
        staged.directory_lock =
            Some(lock_directory(&staged.directory, false).map_err(unavailable)?);
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .share_mode(FILE_SHARE_READ)
            .open(&staged.path)
            .map_err(unavailable)?;
        staged.owns_file = true;
        let identity = file_id(&output).map_err(unavailable)?;
        output.write_all(HELPER).map_err(unavailable)?;
        output.sync_all().map_err(unavailable)?;
        drop(output);
        // The loader cannot execute an image while its writer is open. Reopen
        // read-only, deny writes/deletion, then check both file identity and
        // bytes before showing UAC. Retain the lock until the helper exits.
        let mut image = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&staged.path)
            .map_err(unavailable)?;
        if file_id(&image).map_err(unavailable)? != identity {
            staged.owns_file = false;
            return Err(Issue::Unavailable.into());
        }
        if image.metadata().map_err(unavailable)?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT
            != 0
        {
            return Err(Issue::Unavailable.into());
        }
        let mut bytes = Vec::with_capacity(HELPER.len());
        Read::by_ref(&mut image)
            .take(HELPER.len() as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(unavailable)?;
        if bytes.len() != HELPER.len() || Sha256::digest(&bytes) != Sha256::digest(HELPER) {
            return Err(Issue::Unavailable.into());
        }
        staged.image = Some(image);
        Ok(staged)
    }
}
impl Drop for StagedHelper {
    fn drop(&mut self) {
        self.image.take();
        if self.owns_file {
            let _ = fs::remove_file(&self.path);
        }
        self.directory_lock.take();
        // Nonrecursive: a foreign file is never deleted by cleanup.
        let _ = fs::remove_dir(&self.directory);
    }
}
pub(super) fn launch(action: Action) -> Result<(), Error> {
    // A dedicated STA avoids inheriting a reused blocking worker's COM mode.
    std::thread::Builder::new()
        .name("prism-service-uac".into())
        .spawn(move || launch_on_sta(action))
        .map_err(|error| diagnostic(OperationStage::Worker, error.raw_os_error()))?
        .join()
        .map_err(|_| diagnostic(OperationStage::Worker, None))?
}
fn launch_on_sta(action: Action) -> Result<(), Error> {
    let initialized = unsafe {
        CoInitializeEx(
            std::ptr::null(),
            (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
        )
    };
    if initialized < 0 {
        return Err(diagnostic(OperationStage::Com, Some(initialized)));
    }
    struct Apartment;
    impl Drop for Apartment {
        fn drop(&mut self) {
            unsafe {
                CoUninitialize();
            }
        }
    }
    let _apartment = Apartment;
    let staged = StagedHelper::create()?;
    let image_path = shell_path(&staged.path)?;
    // Shell and filesystem APIs have different namespace rules. Prove the
    // ordinary DOS path still names our locked image before passing it to Shell.
    let shell_image = File::open(&image_path).map_err(unavailable)?;
    if file_id(&shell_image).map_err(unavailable)?
        != file_id(staged.image.as_ref().unwrap()).map_err(unavailable)?
    {
        return Err(Issue::Unavailable.into());
    }
    let (verb, path, arguments, directory) = (
        wide("runas"),
        wide(&image_path),
        wide(action.argument()),
        wide(shell_path(&staged.directory)?),
    );
    let mut execute = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
        lpVerb: verb.as_ptr(),
        lpFile: path.as_ptr(),
        lpParameters: arguments.as_ptr(),
        lpDirectory: directory.as_ptr(),
        nShow: SW_HIDE,
        ..Default::default()
    };
    // One explicit user action reaches this point. No command text, paths or
    // payload come from webview input; only the typed operation is selectable.
    if unsafe { ShellExecuteExW(&mut execute) } == 0 {
        return Err(launch_failure(std::io::Error::last_os_error()));
    }
    if execute.hProcess.is_null() {
        return Err(diagnostic(OperationStage::Launch, None));
    }
    let process = unsafe { OwnedHandle::from_raw_handle(execute.hProcess) };
    if unsafe { WaitForSingleObject(process.as_raw_handle(), INFINITE) } != WAIT_OBJECT_0 {
        return Err(diagnostic(
            OperationStage::Wait,
            std::io::Error::last_os_error().raw_os_error(),
        ));
    }
    let mut code = 0;
    if unsafe { GetExitCodeProcess(process.as_raw_handle(), &mut code) } == 0 {
        return Err(diagnostic(
            OperationStage::ExitCode,
            std::io::Error::last_os_error().raw_os_error(),
        ));
    }
    if code == 0 {
        return Ok(());
    }
    Err(Error {
        code: match code {
            5 => Issue::PermissionDenied,
            80 | 183 | 1073 => Issue::ExistingBinary,
            1058 => Issue::Disabled,
            1223 => Issue::Cancelled,
            1460 => Issue::TimedOut,
            _ => Issue::Failed,
        },
        exit_code: Some(code),
        stage: Some(OperationStage::ExitCode),
        native_code: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_diagnostics_keep_stage_and_code_without_error_text() {
        for (code, issue) in [
            (ERROR_ACCESS_DENIED, Issue::PermissionDenied),
            (ERROR_CANCELLED, Issue::Cancelled),
            (ERROR_FILE_NOT_FOUND, Issue::Unavailable),
        ] {
            let result = launch_failure(std::io::Error::from_raw_os_error(code as i32));
            assert_eq!(result.code, issue);
            assert_eq!(result.native_code, Some(code as i32));
            assert_eq!(serde_json::to_value(result).unwrap()["stage"], "launch");
        }
        let value =
            serde_json::to_value(unavailable(std::io::Error::from_raw_os_error(5))).unwrap();
        assert_eq!(value["stage"], "staging");
        assert_eq!(value["nativeCode"], 5);
        assert_eq!(value["code"], "unavailable");
        let private = unavailable(std::io::Error::other("PRIVATE_HELPER_PATH_SENTINEL"));
        let value = serde_json::to_string(&private).unwrap();
        assert!(!value.contains("PRIVATE_HELPER_PATH_SENTINEL"));
        assert!(!value.contains("nativeCode"));
        assert_eq!(
            serde_json::to_value(diagnostic(OperationStage::Launch, Some(740))).unwrap()["stage"],
            "launch"
        );
    }
    #[test]
    fn real_compiled_helper_is_locked_and_only_owned_files_are_cleaned() {
        let staged = StagedHelper::create().unwrap();
        let (file, directory) = (staged.path.clone(), staged.directory.clone());
        assert!(HELPER.starts_with(b"MZ"));
        assert_eq!(
            file_id(&File::open(shell_path(&file).unwrap()).unwrap()).unwrap(),
            file_id(staged.image.as_ref().unwrap()).unwrap()
        );
        assert!(OpenOptions::new().write(true).open(&file).is_err());
        assert!(fs::rename(&directory, directory.with_extension("moved")).is_err());
        drop(staged);
        assert!(!file.exists());
        assert!(!directory.exists());
    }
    #[test]
    fn shell_paths_preserve_unicode_and_reject_normalization_changes() {
        assert_eq!(
            shell_path(Path::new(r"\\?\C:\문서\helper.exe")).unwrap(),
            PathBuf::from(r"C:\문서\helper.exe")
        );
        for path in [
            r"\\?\C:\folder.\helper.exe",
            r"\\?\C:\folder \helper.exe",
            r"\\server\share\helper.exe",
            "relative.exe",
        ] {
            assert!(shell_path(Path::new(path)).is_err());
        }
    }
}

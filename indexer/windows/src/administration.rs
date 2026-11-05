//! Explicit administrative entry points. Neither the ordinary client nor the
//! SCM host calls these functions. They never request elevation themselves.
use crate::{local_accounts::LocalAccounts, scm, security::Descriptor};
use std::{
    ffi::OsString,
    fs::{self, File},
    io::{self, Write},
    mem::size_of,
    os::windows::{
        ffi::OsStringExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::{Path, PathBuf},
    ptr::{null, null_mut, NonNull},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::*,
    Security::{Authorization::*, *},
    Storage::FileSystem::*,
    System::{
        Com::CoTaskMemFree,
        Services::*,
        SystemServices::{ACCESS_ALLOWED_ACE_TYPE, ACCESS_DENIED_ACE_TYPE},
        Threading::*,
    },
    UI::Shell::{FOLDERID_ProgramFiles, SHGetKnownFolderPath},
};
mod upgrade;
pub use upgrade::upgrade;

const DIRECTORY_ACL: &str = "O:BAG:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FRFX;;;BU)";
const FILE_ACL: &str = "O:BAG:BAD:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;FRFX;;;BU)";
const CONTROL_TIMEOUT: Duration = Duration::from_secs(30);
const READ_SECURITY: u32 = 0x0002_0000;

fn wide(value: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    value.as_ref().encode_wide().chain(Some(0)).collect()
}
fn denied() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "An elevated administrator and protected service location are required.",
    )
}

pub fn is_elevated() -> io::Result<bool> {
    let mut raw = null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let token = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut elevation = TOKEN_ELEVATION::default();
    let mut needed = 0;
    if unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenElevation,
            (&mut elevation as *mut TOKEN_ELEVATION).cast(),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut needed,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(elevation.TokenIsElevated != 0)
}
fn require_administrator() -> io::Result<()> {
    if is_elevated()? {
        Ok(())
    } else {
        Err(denied())
    }
}

fn installation_directory() -> io::Result<PathBuf> {
    let mut pointer = null_mut();
    let result =
        unsafe { SHGetKnownFolderPath(&FOLDERID_ProgramFiles, 0, null_mut(), &mut pointer) };
    if result < 0 || pointer.is_null() {
        return Err(io::Error::other("Could not resolve Program Files."));
    }
    // The shell owns the terminated allocation. Copy before freeing it; no
    // environment variable or caller-supplied directory controls this path.
    let mut length = 0;
    unsafe {
        while *pointer.add(length) != 0 {
            length += 1;
        }
    }
    let path = PathBuf::from(OsString::from_wide(unsafe {
        std::slice::from_raw_parts(pointer, length)
    }));
    unsafe {
        CoTaskMemFree(pointer.cast());
    }
    Ok(path.join("PrismFileIndex"))
}

struct SecurityAllocation(*mut std::ffi::c_void);
impl Drop for SecurityAllocation {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}
fn descriptor_parts(descriptor: PSECURITY_DESCRIPTOR) -> io::Result<(PSID, *mut ACL)> {
    let mut owner = null_mut();
    let mut dacl = null_mut();
    let (mut present, mut defaulted) = (0, 0);
    if unsafe { GetSecurityDescriptorOwner(descriptor, &mut owner, &mut defaulted) } == 0
        || unsafe { GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted) }
            == 0
        || present == 0
        || owner.is_null()
        || dacl.is_null()
    {
        return Err(denied());
    }
    Ok((owner, dacl))
}
fn same_acl(left: *mut ACL, right: *mut ACL) -> io::Result<bool> {
    // Compare ACEs, not allocator padding or unused ACL space.
    if unsafe { (*left).AceCount != (*right).AceCount } {
        return Ok(false);
    }
    for index in 0..unsafe { (*left).AceCount } {
        let (mut a, mut b) = (null_mut(), null_mut());
        if unsafe { GetAce(left, index as u32, &mut a) } == 0
            || unsafe { GetAce(right, index as u32, &mut b) } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let a_length = unsafe { (*(a as *const ACE_HEADER)).AceSize } as usize;
        let b_length = unsafe { (*(b as *const ACE_HEADER)).AceSize } as usize;
        if a_length != b_length
            || unsafe {
                std::slice::from_raw_parts(a.cast::<u8>(), a_length)
                    != std::slice::from_raw_parts(b.cast::<u8>(), b_length)
            }
        {
            return Ok(false);
        }
    }
    Ok(true)
}
fn protected_handle(handle: &OwnedHandle, expected: &Descriptor) -> io::Result<()> {
    let mut descriptor = null_mut();
    let result = unsafe {
        GetSecurityInfo(
            handle.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
            &mut descriptor,
        )
    };
    if result != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(result as i32));
    }
    let allocation = SecurityAllocation(descriptor);
    let (owner, dacl) = descriptor_parts(allocation.0)?;
    let (expected_owner, expected_dacl) = descriptor_parts(expected.raw())?;
    let (mut control, mut revision) = (0, 0);
    if unsafe { GetSecurityDescriptorControl(allocation.0, &mut control, &mut revision) } == 0
        || control & SE_DACL_PROTECTED == 0
        || unsafe { EqualSid(owner, expected_owner) } == 0
        || !same_acl(dacl, expected_dacl)?
    {
        return Err(denied());
    }
    Ok(())
}
fn open_metadata(path: &Path, directory: bool) -> io::Result<OwnedHandle> {
    // Deny deletion/renaming while checking the actual handle. Do not follow
    // final-component reparse points, and never inherit this handle.
    let raw = unsafe {
        CreateFileW(
            wide(path).as_ptr(),
            READ_SECURITY | FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT
                | if directory {
                    FILE_FLAG_BACKUP_SEMANTICS
                } else {
                    0
                },
            null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut info = FILE_ATTRIBUTE_TAG_INFO::default();
    if unsafe {
        GetFileInformationByHandleEx(
            handle.as_raw_handle(),
            FileAttributeTagInfo,
            (&mut info as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
            size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    if info.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || (info.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0) != directory
    {
        return Err(denied());
    }
    Ok(handle)
}

fn installer_sid() -> io::Result<Vec<usize>> {
    let name = wide(r"NT SERVICE\TrustedInstaller");
    let (mut sid_length, mut domain_length, mut kind) = (0, 0, 0);
    unsafe {
        LookupAccountNameW(
            null(),
            name.as_ptr(),
            null_mut(),
            &mut sid_length,
            null_mut(),
            &mut domain_length,
            &mut kind,
        );
    }
    if sid_length == 0 || sid_length > 1024 || domain_length > 1024 {
        return Err(denied());
    }
    let mut sid = vec![0usize; (sid_length as usize).div_ceil(size_of::<usize>())];
    let mut domain = vec![0u16; domain_length as usize];
    if unsafe {
        LookupAccountNameW(
            null(),
            name.as_ptr(),
            sid.as_mut_ptr().cast(),
            &mut sid_length,
            domain.as_mut_ptr(),
            &mut domain_length,
            &mut kind,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(sid)
}
fn trusted_sid(sid: PSID, installer: &[usize]) -> bool {
    unsafe {
        IsWellKnownSid(sid, WinLocalSystemSid) != 0
            || IsWellKnownSid(sid, WinBuiltinAdministratorsSid) != 0
            || EqualSid(sid, installer.as_ptr().cast_mut().cast()) != 0
    }
}
fn trusted_ancestor(
    handle: &OwnedHandle,
    installer: &[usize],
    accounts: &LocalAccounts,
) -> io::Result<()> {
    let mut descriptor = null_mut();
    let result = unsafe {
        GetSecurityInfo(
            handle.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
            &mut descriptor,
        )
    };
    if result != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(result as i32));
    }
    let descriptor = SecurityAllocation(descriptor);
    trusted_ancestor_descriptor(descriptor.0, installer, accounts)
}
fn trusted_ancestor_descriptor(
    descriptor: PSECURITY_DESCRIPTOR,
    installer: &[usize],
    accounts: &LocalAccounts,
) -> io::Result<()> {
    let (owner, dacl) = descriptor_parts(descriptor)?;
    if !trusted_sid(owner, installer) {
        return Err(denied());
    }
    // Creating a sibling under C:\ is harmless; changing a protected existing
    // child's name, ACL, owner or parent reparse data is not. Inherit-only ACEs
    // apply to children, not to the directory handle checked here.
    const UNSAFE: u32 = WRITE_DAC
        | WRITE_OWNER
        | DELETE
        | FILE_DELETE_CHILD
        | FILE_WRITE_ATTRIBUTES
        | FILE_WRITE_EA
        | GENERIC_ALL
        | GENERIC_WRITE;
    for index in 0..unsafe { (*dacl).AceCount } {
        let mut raw = null_mut();
        if unsafe { GetAce(dacl, index as u32, &mut raw) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let header = unsafe { &*raw.cast::<ACE_HEADER>() };
        if header.AceFlags & INHERIT_ONLY_ACE as u8 != 0 {
            continue;
        }
        match header.AceType as u32 {
            ACCESS_DENIED_ACE_TYPE => {}
            ACCESS_ALLOWED_ACE_TYPE => {
                let ace = unsafe { &*raw.cast::<ACCESS_ALLOWED_ACE>() };
                let sid = (&ace.SidStart as *const u32).cast_mut().cast();
                if ace.Mask & UNSAFE != 0
                    && !trusted_sid(sid, installer)
                    && !accounts.absent_grantee(sid)
                {
                    return Err(denied());
                }
            }
            // Conditional/object-specific grants need a separate access policy;
            // fail closed rather than guessing whether a grant is safe.
            _ => return Err(denied()),
        }
    }
    Ok(())
}
fn lock_ancestors(parent: &Path) -> io::Result<Vec<OwnedHandle>> {
    use std::path::{Component, Prefix};
    let installer = installer_sid()?;
    let accounts = LocalAccounts::read();
    let mut path = PathBuf::new();
    let mut handles = Vec::new();
    for component in parent.components() {
        match component {
            Component::Prefix(prefix) if matches!(prefix.kind(), Prefix::Disk(_)) => {
                path.push(prefix.as_os_str());
                continue;
            }
            Component::RootDir | Component::Normal(_) => path.push(component.as_os_str()),
            _ => return Err(denied()),
        }
        let handle = open_metadata(&path, true)?;
        trusted_ancestor(&handle, &installer, &accounts)?;
        handles.push(handle);
    }
    if handles.is_empty() {
        return Err(denied());
    }
    Ok(handles)
}

struct ServiceHandle(NonNull<std::ffi::c_void>);
impl ServiceHandle {
    fn new(raw: SC_HANDLE) -> io::Result<Self> {
        Ok(Self(
            NonNull::new(raw).ok_or_else(io::Error::last_os_error)?,
        ))
    }
    fn raw(&self) -> SC_HANDLE {
        self.0.as_ptr()
    }
}
impl Drop for ServiceHandle {
    fn drop(&mut self) {
        unsafe {
            CloseServiceHandle(self.raw());
        }
    }
}
fn service_state(service: &ServiceHandle) -> io::Result<u32> {
    let mut state = SERVICE_STATUS::default();
    if unsafe { QueryServiceStatus(service.raw(), &mut state) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(state.dwCurrentState)
}
fn wait_state(service: &ServiceHandle, expected: u32) -> io::Result<()> {
    let start = Instant::now();
    while service_state(service)? != expected {
        if start.elapsed() > CONTROL_TIMEOUT {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Service state change timed out.",
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}
fn stop(service: &ServiceHandle) -> io::Result<()> {
    if service_state(service)? == SERVICE_STOPPED {
        return Ok(());
    }
    let mut state = SERVICE_STATUS::default();
    if unsafe { ControlService(service.raw(), SERVICE_CONTROL_STOP, &mut state) } == 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(ERROR_SERVICE_NOT_ACTIVE as i32) {
            return Err(error);
        }
    }
    wait_state(service, SERVICE_STOPPED)
}
fn command(path: &Path) -> String {
    format!("\"{}\" --service", path.display())
}
fn file_identity(handle: &OwnedHandle) -> io::Result<FILE_ID_INFO> {
    let mut identity = FILE_ID_INFO::default();
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
    Ok(identity)
}
fn configuration_command(buffer: &[usize], pointer: *const u16) -> io::Result<String> {
    let begin = buffer.as_ptr() as usize;
    let end = begin
        .checked_add(std::mem::size_of_val(buffer))
        .ok_or_else(denied)?;
    let address = pointer as usize;
    if address < begin || address >= end || !address.is_multiple_of(2) {
        return Err(denied());
    }
    // The checked pointer lies inside this initialized aligned output buffer.
    // Never scan beyond its end, even if the service configuration is invalid.
    let units = unsafe { std::slice::from_raw_parts(pointer, (end - address) / 2) };
    let length = units
        .iter()
        .position(|unit| *unit == 0)
        .ok_or_else(denied)?;
    String::from_utf16(&units[..length]).map_err(|_| denied())
}

/// Ordinary-user preflight for the settings UI. Query only; callers can report
/// a path policy failure before requesting UAC. Installation still repeats
/// these checks while retaining its own directory guards across all writes.
pub fn check_install_location() -> io::Result<()> {
    let directory = installation_directory()?;
    let _parents = lock_ancestors(directory.parent().ok_or_else(denied)?)?;
    match open_metadata(&directory, true) {
        Ok(handle) => {
            protected_handle(&handle, &Descriptor::parse(DIRECTORY_ACL)?)?;
            match fs::symlink_metadata(directory.join("prism-file-index.exe")) {
                Ok(_) => Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "An existing service executable will not be replaced by initial installation.",
                )),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error),
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// Initial optional installation only. An existing service or unknown binary
/// is left untouched; replacement/update policy is intentionally separate.
pub fn install(payload: &[u8]) -> io::Result<()> {
    require_administrator()?;
    validate_payload(payload)?;
    let manager = ServiceHandle::new(unsafe {
        OpenSCManagerW(
            null(),
            null(),
            SC_MANAGER_CONNECT | SC_MANAGER_CREATE_SERVICE,
        )
    })?;
    let name = wide(scm::SERVICE_NAME);
    let existing = unsafe { OpenServiceW(manager.raw(), name.as_ptr(), SERVICE_QUERY_STATUS) };
    if !existing.is_null() {
        drop(ServiceHandle::new(existing)?);
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "The filename service is already installed.",
        ));
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() != Some(ERROR_SERVICE_DOES_NOT_EXIST as i32) {
        return Err(error);
    }
    let directory = installation_directory()?;
    let _parents = lock_ancestors(directory.parent().ok_or_else(denied)?)?;
    let descriptor = Descriptor::parse(DIRECTORY_ACL)?;
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
    let directory_handle = open_metadata(&directory, true)?;
    protected_handle(&directory_handle, &descriptor)?;
    let target = directory.join("prism-file-index.exe");
    let file_descriptor = Descriptor::parse(FILE_ACL)?;
    let attributes = SECURITY_ATTRIBUTES {
        lpSecurityDescriptor: file_descriptor.raw(),
        ..attributes
    };
    let raw = unsafe {
        CreateFileW(
            wide(&target).as_ptr(),
            GENERIC_WRITE,
            FILE_SHARE_READ,
            &attributes,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // Only this newly created file belongs to this transaction. Never replace
    // or recursively delete a pre-existing file or directory.
    let result = (|| {
        let mut destination = unsafe { File::from_raw_handle(raw) };
        destination.write_all(payload)?;
        destination.sync_all()?;
        drop(destination);
        protected_handle(&open_metadata(&target, false)?, &file_descriptor)?;
        let service = ServiceHandle::new(unsafe {
            CreateServiceW(
                manager.raw(),
                name.as_ptr(),
                wide(scm::DISPLAY_NAME).as_ptr(),
                SERVICE_QUERY_STATUS | SERVICE_START | SERVICE_STOP | DELETE,
                SERVICE_WIN32_OWN_PROCESS,
                SERVICE_AUTO_START,
                SERVICE_ERROR_NORMAL,
                wide(command(&target)).as_ptr(),
                null(),
                null_mut(),
                null(),
                null(),
                null(),
            )
        })?;
        let started = if unsafe { StartServiceW(service.raw(), 0, null()) } == 0 {
            Err(io::Error::last_os_error())
        } else {
            wait_state(&service, SERVICE_RUNNING)
        };
        if let Err(error) = started {
            let _ = stop(&service);
            unsafe {
                DeleteService(service.raw());
            }
            return Err(error);
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&target);
    }
    result
}

struct VerifiedService {
    _manager: ServiceHandle,
    service: ServiceHandle,
    directory: PathBuf,
    target: PathBuf,
    directory_handle: OwnedHandle,
    _parents: Vec<OwnedHandle>,
}
fn verified_service(access: u32) -> io::Result<VerifiedService> {
    let manager =
        ServiceHandle::new(unsafe { OpenSCManagerW(null(), null(), SC_MANAGER_CONNECT) })?;
    let service = ServiceHandle::new(unsafe {
        OpenServiceW(
            manager.raw(),
            wide(scm::SERVICE_NAME).as_ptr(),
            SERVICE_QUERY_CONFIG | SERVICE_QUERY_STATUS | access,
        )
    })?;
    let directory = installation_directory()?;
    let parents = lock_ancestors(directory.parent().ok_or_else(denied)?)?;
    let directory_handle = open_metadata(&directory, true)?;
    protected_handle(&directory_handle, &Descriptor::parse(DIRECTORY_ACL)?)?;
    let target = directory.join("prism-file-index.exe");
    protected_handle(
        &open_metadata(&target, false)?,
        &Descriptor::parse(FILE_ACL)?,
    )?;
    let mut needed = 0;
    // A Vec of usize supplies the alignment required by QUERY_SERVICE_CONFIGW.
    unsafe {
        QueryServiceConfigW(service.raw(), null_mut(), 0, &mut needed);
    }
    if needed == 0 || needed > 65536 {
        return Err(denied());
    }
    let mut buffer = vec![0usize; (needed as usize).div_ceil(size_of::<usize>())];
    let config = buffer.as_mut_ptr().cast::<QUERY_SERVICE_CONFIGW>();
    if unsafe { QueryServiceConfigW(service.raw(), config, needed, &mut needed) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let binary = configuration_command(&buffer, unsafe { (*config).lpBinaryPathName })?;
    if !binary.eq_ignore_ascii_case(&command(&target))
        || unsafe { (*config).dwServiceType } != SERVICE_WIN32_OWN_PROCESS
        || !configuration_command(&buffer, unsafe { (*config).lpServiceStartName })?
            .eq_ignore_ascii_case("LocalSystem")
    {
        return Err(denied());
    }
    Ok(VerifiedService {
        _manager: manager,
        service,
        directory,
        target,
        directory_handle,
        _parents: parents,
    })
}
/// Read-only ownership check before the app offers a start or removal action.
pub fn check_owned_service() -> io::Result<()> {
    verified_service(0).map(|_| ())
}
/// Explicit start only. Ordinary app startup never calls this function.
pub fn start() -> io::Result<()> {
    require_administrator()?;
    let verified = verified_service(SERVICE_START)?;
    match service_state(&verified.service)? {
        SERVICE_RUNNING => return Ok(()),
        SERVICE_START_PENDING => return wait_state(&verified.service, SERVICE_RUNNING),
        SERVICE_STOP_PENDING => wait_state(&verified.service, SERVICE_STOPPED)?,
        _ => {}
    }
    if unsafe { StartServiceW(verified.service.raw(), 0, null()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    wait_state(&verified.service, SERVICE_RUNNING)
}
/// Remove only the exact protected service. The parent directory guards stay
/// alive across deletion; unrelated files prevent directory removal and remain.
pub fn uninstall() -> io::Result<()> {
    require_administrator()?;
    let verified = verified_service(SERVICE_STOP | DELETE)?;
    let current = file_identity(&open_metadata(&std::env::current_exe()?, false)?)?;
    let installed = file_identity(&open_metadata(&verified.target, false)?)?;
    if current.VolumeSerialNumber == installed.VolumeSerialNumber
        && current.FileId.Identifier == installed.FileId.Identifier
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Remove the service through the dedicated management helper.",
        ));
    }
    stop(&verified.service)?;
    if unsafe { DeleteService(verified.service.raw()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    drop(verified.service);
    // SCM can report STOPPED just before the process image is released. Give
    // that image/antivirus lock a bounded grace period; never kill by PID/name.
    remove_after_stop(&verified.target, Duration::from_secs(5))?;
    drop(verified.directory_handle);
    match fs::remove_dir(&verified.directory) {
        Ok(()) => Ok(()),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::DirectoryNotEmpty | io::ErrorKind::NotFound
            ) || matches!(error.raw_os_error(), Some(5 | 32 | 33)) =>
        {
            Ok(())
        }
        Err(error) => Err(error),
    }
}
fn remove_after_stop(path: &Path, limit: Duration) -> io::Result<()> {
    let start = Instant::now();
    loop {
        match fs::remove_file(path) {
            Ok(()) => return Ok(()),
            Err(error) if matches!(error.raw_os_error(), Some(5 | 32 | 33)) => {
                if start.elapsed() >= limit {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "Service executable is still in use.",
                    ));
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => return Err(error),
        }
    }
}
/// Stable process results consumed by the ordinary app; no raw path or system
/// diagnostic crosses the elevation boundary through an error message.
pub fn exit_code(error: &io::Error) -> i32 {
    if let Some(code) = error.raw_os_error().filter(|code| *code > 0) {
        return code;
    }
    match error.kind() {
        io::ErrorKind::PermissionDenied => 5,
        io::ErrorKind::AlreadyExists => 183,
        io::ErrorKind::InvalidInput | io::ErrorKind::InvalidData => 87,
        io::ErrorKind::NotFound => 2,
        io::ErrorKind::TimedOut => 1460,
        io::ErrorKind::Unsupported => 50,
        _ => 1,
    }
}
/// Sanity-check the compiled service image without opening files or SCM.
pub fn validate_payload(bytes: &[u8]) -> io::Result<()> {
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid embedded service executable.",
        )
    };
    if !(64..=16 * 1024 * 1024).contains(&bytes.len()) || !bytes.starts_with(b"MZ") {
        return Err(invalid());
    }
    let offset = u32::from_le_bytes(bytes[60..64].try_into().unwrap()) as usize;
    if offset < 64 {
        return Err(invalid());
    }
    let header = bytes
        .get(offset..offset.checked_add(26).ok_or_else(invalid)?)
        .ok_or_else(invalid)?;
    let machine = if cfg!(target_arch = "x86_64") {
        0x8664u16
    } else if cfg!(target_arch = "aarch64") {
        0xaa64
    } else {
        0x14c
    };
    let magic = if cfg!(target_pointer_width = "64") {
        0x20bu16
    } else {
        0x10b
    };
    if &header[..4] != b"PE\0\0"
        || header[4..6] != machine.to_le_bytes()
        || header[24..26] != magic.to_le_bytes()
    {
        return Err(invalid());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deletion_waits_for_an_owned_file_lock_and_preserves_it_on_timeout() {
        use std::os::windows::fs::OpenOptionsExt;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("service-image-fixture.bin");
        fs::write(&path, b"public fixture").unwrap();
        let file = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(&path)
            .unwrap();
        assert_eq!(
            remove_after_stop(&path, Duration::from_millis(5))
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut
        );
        assert!(path.exists());
        let holder = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(60));
            drop(file);
        });
        remove_after_stop(&path, Duration::from_secs(2)).unwrap();
        holder.join().unwrap();
        assert!(!path.exists());
    }
    #[test]
    fn corrupt_embedded_images_are_rejected_before_deployment() {
        for bytes in [vec![], vec![0; 64], {
            let mut value = vec![0; 64];
            value[..2].copy_from_slice(b"MZ");
            value[60..64].copy_from_slice(&u32::MAX.to_le_bytes());
            value
        }] {
            assert_eq!(
                validate_payload(&bytes).unwrap_err().kind(),
                io::ErrorKind::InvalidData
            );
        }
        assert_eq!(exit_code(&denied()), 5);
        assert_eq!(
            exit_code(&io::Error::new(io::ErrorKind::TimedOut, "")),
            1460
        );
        assert_eq!(exit_code(&io::Error::from_raw_os_error(1058)), 1058);
    }
    #[test]
    fn unprivileged_management_is_rejected_before_any_installation_work() {
        if is_elevated().unwrap() {
            return;
        }
        assert_eq!(
            install(&[]).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert_eq!(
            uninstall().unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
    }
    #[test]
    fn service_command_quotes_the_fixed_executable_and_has_no_extra_arguments() {
        assert_eq!(
            command(Path::new(
                r"C:\Program Files\PrismFileIndex\prism-file-index.exe"
            )),
            r#""C:\Program Files\PrismFileIndex\prism-file-index.exe" --service"#
        );
    }
    #[test]
    fn ordinary_owned_temporary_directories_are_not_trusted_service_locations() {
        let directory = tempfile::tempdir().unwrap();
        let handle = open_metadata(directory.path(), true).unwrap();
        assert_eq!(
            protected_handle(&handle, &Descriptor::parse(DIRECTORY_ACL).unwrap())
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
    }
    #[test]
    fn actual_program_files_ancestors_are_checked_without_mutation() {
        let directory = installation_directory().unwrap();
        match lock_ancestors(directory.parent().unwrap()) {
            Ok(guards) => assert!(guards.len() >= 2),
            // A customized machine ACL can legitimately fail the policy.
            // Never change a system directory ACL to make installation work.
            Err(error) => assert_eq!(error.kind(), io::ErrorKind::PermissionDenied),
        }
    }
    #[test]
    fn ancestor_policy_rejects_unsafe_grants_and_untrusted_owners() {
        let installer = installer_sid().unwrap();
        for (sddl, allowed) in [
            ("O:SYD:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;FRFX;;;BU)", true),
            ("O:BAD:P(A;;FA;;;BA)(A;;0x4;;;BU)", true),
            ("O:BAD:P(A;OICIIO;FA;;;CO)(A;;FRFX;;;BU)", true),
            ("O:BAD:P(D;;FA;;;BU)(A;;FA;;;BA)", true),
            ("O:BAD:P(A;;FA;;;BU)", false),
            ("O:BAD:P(A;;WD;;;AU)", false),
            ("O:BAD:P(A;;WO;;;WD)", false),
            ("O:BAD:P(A;;0x40;;;BU)", false),
            ("O:BUD:P(A;;FRFX;;;BU)", false),
        ] {
            let descriptor = Descriptor::parse(sddl).unwrap();
            assert_eq!(
                trusted_ancestor_descriptor(
                    descriptor.raw(),
                    &installer,
                    &LocalAccounts::unavailable()
                )
                .is_ok(),
                allowed,
                "{sddl}"
            );
        }
    }
    #[test]
    fn configuration_strings_are_bounded_by_the_owned_query_buffer() {
        let mut buffer = vec![0usize; 4];
        let units = unsafe {
            std::slice::from_raw_parts_mut(
                buffer.as_mut_ptr().cast::<u16>(),
                std::mem::size_of_val(buffer.as_slice()) / 2,
            )
        };
        units[..3].copy_from_slice(&[b'a' as u16, b'b' as u16, 0]);
        assert_eq!(
            configuration_command(&buffer, buffer.as_ptr().cast()).unwrap(),
            "ab"
        );
        assert!(configuration_command(&buffer, null()).is_err());
        assert!(
            configuration_command(&buffer, (buffer.as_ptr() as usize + 1) as *const u16).is_err()
        );
        let end = unsafe { buffer.as_ptr().add(buffer.len()) }.cast::<u16>();
        assert!(configuration_command(&buffer, end).is_err());
        buffer.fill(usize::MAX);
        assert!(configuration_command(&buffer, buffer.as_ptr().cast()).is_err());
    }
}

//! Consent-bound, exact-image graceful shutdown. No basename kills, forced
//! termination, file/service Restart Manager registration or automatic restart.
use std::{io, path::Path};

/// A running application is left untouched unless the caller carries explicit
/// close consent. A close request never authorizes another account/session.
pub fn ensure_closed(app: &Path, allow_close: bool) -> io::Result<()> {
    #[cfg(windows)]
    {
        native::ensure_closed(app, allow_close).map_err(close_failed)
    }
    #[cfg(not(windows))]
    {
        let _ = (app, allow_close);
        Ok(())
    }
}

#[cfg(any(windows, test))]
fn close_failed(error: io::Error) -> io::Error {
    if error.to_string().starts_with("APP_RUNNING:")
        || error.to_string().starts_with("APP_CLOSE_FAILED:")
    {
        error
    } else {
        io::Error::new(error.kind(), format!("APP_CLOSE_FAILED: {error}"))
    }
}

#[cfg(any(windows, test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Identity {
    pid: u32,
    created: u64,
}

#[cfg(any(windows, test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileIdentity {
    volume: u64,
    index: [u8; 16],
}

#[cfg(any(windows, test))]
fn same_file(expected: Option<FileIdentity>, actual: FileIdentity) -> bool {
    expected.is_some_and(|expected| expected.index != [0; 16] && expected == actual)
}

#[cfg(any(windows, test))]
fn check_consent(scopes: &[bool], allow_close: bool) -> io::Result<bool> {
    if scopes.is_empty() {
        return Ok(false);
    }
    if !allow_close {
        return Err(io::Error::other(
            "APP_RUNNING: Prism is running. Close it or explicitly allow setup to request a graceful close.",
        ));
    }
    if scopes.iter().any(|allowed| !allowed) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "APP_CLOSE_FAILED: A matching application belongs to another account/session, is the installer itself, or its identity cannot be verified.",
        ));
    }
    Ok(true)
}

#[cfg(any(windows, test))]
fn check_affected(expected: &[Identity], actual: &[(Identity, bool)]) -> io::Result<()> {
    let mut seen = std::collections::HashSet::new();
    for (identity, allowed_session_and_type) in actual {
        if !allowed_session_and_type || !expected.contains(identity) || !seen.insert(*identity) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Restart Manager returned an unexpected process identity/session/type; no expanded shutdown is authorized.",
            ));
        }
    }
    Ok(())
}

#[cfg(windows)]
mod native {
    use super::*;
    use std::{
        ffi::OsString,
        fs,
        mem::size_of,
        os::windows::ffi::{OsStrExt, OsStringExt},
        path::PathBuf,
        time::{Duration, Instant},
    };
    use windows::{
        core::{PCWSTR, PWSTR},
        Win32::{
            Foundation::{
                CloseHandle, ERROR_INSUFFICIENT_BUFFER, ERROR_INVALID_PARAMETER, ERROR_MORE_DATA,
                ERROR_NO_MORE_FILES, ERROR_SUCCESS, FILETIME, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT,
            },
            Globalization::{CompareStringOrdinal, CSTR_EQUAL},
            Security::{
                GetLengthSid, GetTokenInformation, IsValidSid, TokenUser, TOKEN_QUERY, TOKEN_USER,
            },
            Storage::FileSystem::{
                CreateFileW, FileAttributeTagInfo, FileIdInfo, GetFileInformationByHandleEx,
                FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO,
                FILE_FLAG_OPEN_REPARSE_POINT, FILE_FLAG_POSIX_SEMANTICS, FILE_ID_INFO,
                FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
                OPEN_EXISTING,
            },
            System::{
                Diagnostics::ToolHelp::{
                    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
                    TH32CS_SNAPPROCESS,
                },
                RemoteDesktop::ProcessIdToSessionId,
                RestartManager::{
                    RmCritical, RmEndSession, RmGetList, RmRegisterResources, RmService,
                    RmShutdown, RmStartSession, CCH_RM_SESSION_KEY, RM_PROCESS_INFO,
                    RM_UNIQUE_PROCESS,
                },
                Threading::{
                    GetCurrentProcess, GetCurrentProcessId, GetProcessId, GetProcessTimes,
                    OpenProcess, OpenProcessToken, QueryFullProcessImageNameW, WaitForSingleObject,
                    PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
                },
            },
        },
    };

    const MAX_TARGETS: usize = 32;
    const WAIT_BUDGET: Duration = Duration::from_secs(15);

    struct Handle(HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: every Handle owns one successful, non-pseudo Win32 handle.
            let _ = unsafe { CloseHandle(self.0) };
        }
    }
    struct ImageFile {
        handle: Handle,
        identity: FileIdentity,
    }
    impl ImageFile {
        fn open(original_path: &Path) -> io::Result<Self> {
            // Do not canonicalize first: in a Windows case-sensitive directory
            // that could substitute a distinct, differently cased executable.
            crate::safe_path::no_links(original_path)?;
            let mut name: Vec<u16> = original_path.as_os_str().encode_wide().collect();
            for ch in &mut name {
                if *ch == b'/' as u16 {
                    *ch = b'\\' as u16;
                }
            }
            let prefix: Vec<u16> = r"\\?\".encode_utf16().collect();
            if !name.starts_with(&prefix) {
                if name.starts_with(&[b'\\' as u16, b'\\' as u16]) {
                    name = [
                        r"\\?\UNC\".encode_utf16().collect::<Vec<_>>(),
                        name[2..].to_vec(),
                    ]
                    .concat();
                } else {
                    name = [prefix, name].concat();
                }
            }
            name.push(0);
            // Only metadata access; all sharing remains allowed. POSIX semantics
            // preserves original spelling even in case-sensitive directories.
            let raw = unsafe {
                CreateFileW(
                    PCWSTR(name.as_ptr()),
                    FILE_READ_ATTRIBUTES.0,
                    FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                    None,
                    OPEN_EXISTING,
                    FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_POSIX_SEMANTICS,
                    None,
                )
            }
            .map_err(|error| {
                io::Error::new(
                    io::Error::from_raw_os_error(win_code(&error) as i32).kind(),
                    format!("Open exact image file identity: {error}"),
                )
            })?;
            let handle = Handle(raw);
            let identity = Self::read(handle.0)?;
            Ok(Self { handle, identity })
        }
        fn read(handle: HANDLE) -> io::Result<FileIdentity> {
            let mut attributes = FILE_ATTRIBUTE_TAG_INFO::default();
            // SAFETY: retained file handle and exactly sized writable structure.
            unsafe {
                GetFileInformationByHandleEx(
                    handle,
                    FileAttributeTagInfo,
                    (&mut attributes as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
                    size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
                )
            }
            .map_err(|error| api_error("GetFileInformationByHandleEx(image attributes)", error))?;
            if attributes.FileAttributes
                & (FILE_ATTRIBUTE_DIRECTORY.0 | FILE_ATTRIBUTE_REPARSE_POINT.0)
                != 0
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "Image identity is not a plain executable file.",
                ));
            }
            let mut info = FILE_ID_INFO::default();
            // ReFS requires the full 128-bit file ID and 64-bit volume serial;
            // the legacy BY_HANDLE_FILE_INFORMATION index can alias files.
            // SAFETY: retained handle and exactly sized FILE_ID_INFO buffer.
            unsafe {
                GetFileInformationByHandleEx(
                    handle,
                    FileIdInfo,
                    (&mut info as *mut FILE_ID_INFO).cast(),
                    size_of::<FILE_ID_INFO>() as u32,
                )
            }
            .map_err(|error| api_error("GetFileInformationByHandleEx(FileIdInfo/image)", error))?;
            let identity = FileIdentity {
                volume: info.VolumeSerialNumber,
                index: info.FileId.Identifier,
            };
            if identity.index == [0; 16] {
                return Err(io::Error::other(
                    "Filesystem did not provide a usable executable identity.",
                ));
            }
            Ok(identity)
        }
    }
    struct Application {
        path: PathBuf,
        image: Option<ImageFile>,
    }
    impl Application {
        fn new(original_path: &Path) -> io::Result<Self> {
            crate::safe_path::no_links(original_path)?;
            let image = match ImageFile::open(original_path) {
                Ok(image) => Some(image),
                Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                Err(error) => return Err(error),
            };
            Ok(Self {
                path: resolved(original_path)?,
                image,
            })
        }
        fn identity(&self) -> io::Result<Option<FileIdentity>> {
            self.image
                .as_ref()
                .map(|image| {
                    let current = ImageFile::read(image.handle.0)?;
                    if current != image.identity {
                        return Err(io::Error::other("Retained target image identity changed."));
                    }
                    Ok(current)
                })
                .transpose()
        }
    }
    fn api_error(action: &str, error: windows::core::Error) -> io::Error {
        io::Error::other(format!("{action}: {error}"))
    }
    fn win_code(error: &windows::core::Error) -> u32 {
        error.code().0 as u32 & 0xffff
    }
    fn session_id(pid: u32) -> io::Result<u32> {
        let mut id = 0;
        // SAFETY: output points to writable scalar storage.
        unsafe { ProcessIdToSessionId(pid, &mut id) }
            .map_err(|e| api_error("ProcessIdToSessionId", e))?;
        Ok(id)
    }
    fn user_sid(process: HANDLE) -> io::Result<Vec<u8>> {
        let mut token = HANDLE::default();
        // SAFETY: process is borrowed and the output token is exclusively owned.
        unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) }
            .map_err(|e| api_error("OpenProcessToken(TOKEN_QUERY)", e))?;
        let token = Handle(token);
        let mut needed = 0;
        // SAFETY: the first call only queries the required byte count.
        let first = unsafe { GetTokenInformation(token.0, TokenUser, None, 0, &mut needed) };
        if first
            .as_ref()
            .err()
            .is_some_and(|e| win_code(e) != ERROR_INSUFFICIENT_BUFFER.0)
            || needed < size_of::<TOKEN_USER>() as u32
            || needed > 65536
        {
            return Err(io::Error::other(format!(
                "TokenUser size query failed: {first:?}, size={needed}"
            )));
        }
        // usize storage provides TOKEN_USER's required pointer alignment.
        let mut buffer = vec![0usize; (needed as usize).div_ceil(size_of::<usize>())];
        // SAFETY: aligned buffer contains at least `needed` writable bytes.
        unsafe {
            GetTokenInformation(
                token.0,
                TokenUser,
                Some(buffer.as_mut_ptr().cast()),
                needed,
                &mut needed,
            )
        }
        .map_err(|e| api_error("GetTokenInformation(TokenUser)", e))?;
        // SAFETY: a successful TokenUser request initializes this header.
        let sid = unsafe { (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid };
        let start = buffer.as_ptr() as usize;
        let end = start + buffer.len() * size_of::<usize>();
        let address = sid.0 as usize;
        if address < start || address.checked_add(8).is_none_or(|value| value > end) {
            return Err(io::Error::other(
                "TokenUser returned an invalid SID pointer.",
            ));
        }
        // SAFETY: SID points into the returned token buffer; validate before copying.
        if !unsafe { IsValidSid(sid) }.as_bool() {
            return Err(io::Error::other("TokenUser returned an invalid SID."));
        }
        let len = unsafe { GetLengthSid(sid) } as usize;
        if !(8..=68).contains(&len) || address.checked_add(len).is_none_or(|value| value > end) {
            return Err(io::Error::other("TokenUser SID exceeds its buffer."));
        }
        // SAFETY: the complete validated SID is inside the live aligned buffer.
        Ok(unsafe { std::slice::from_raw_parts(sid.0.cast::<u8>(), len) }.to_vec())
    }
    fn created(process: HANDLE) -> io::Result<u64> {
        let (mut birth, mut exit, mut kernel, mut user) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        // SAFETY: all outputs are initialized writable FILETIME structures.
        unsafe { GetProcessTimes(process, &mut birth, &mut exit, &mut kernel, &mut user) }
            .map_err(|e| api_error("GetProcessTimes", e))?;
        Ok(u64::from(birth.dwLowDateTime) | (u64::from(birth.dwHighDateTime) << 32))
    }
    fn alive(process: HANDLE) -> io::Result<bool> {
        // SAFETY: retained handle has SYNCHRONIZE; a zero wait never blocks.
        match unsafe { WaitForSingleObject(process, 0) } {
            WAIT_OBJECT_0 => Ok(false),
            WAIT_TIMEOUT => Ok(true),
            _ => Err(io::Error::last_os_error()),
        }
    }
    fn image(process: HANDLE) -> io::Result<PathBuf> {
        let mut buffer = vec![0u16; 32768];
        let mut count = buffer.len() as u32;
        // SAFETY: writable UTF-16 output has exactly the advertised capacity.
        unsafe {
            QueryFullProcessImageNameW(
                process,
                PROCESS_NAME_WIN32,
                PWSTR(buffer.as_mut_ptr()),
                &mut count,
            )
        }
        .map_err(|e| api_error("QueryFullProcessImageNameW", e))?;
        if count == 0 || count as usize > buffer.len() {
            return Err(io::Error::other("Invalid process image path length."));
        }
        Ok(PathBuf::from(OsString::from_wide(
            &buffer[..count as usize],
        )))
    }
    fn wide_key(path: &Path) -> Vec<u16> {
        let mut text: Vec<_> = path.as_os_str().encode_wide().collect();
        for ch in &mut text {
            if *ch == b'/' as u16 {
                *ch = b'\\' as u16;
            }
        }
        let unc: Vec<u16> = r"\\?\UNC\".encode_utf16().collect();
        let extended: Vec<u16> = r"\\?\".encode_utf16().collect();
        if text.len() >= unc.len()
            && unsafe { CompareStringOrdinal(&text[..unc.len()], &unc, true) } == CSTR_EQUAL
        {
            [vec![b'\\' as u16; 2], text[unc.len()..].to_vec()].concat()
        } else if text.starts_with(&extended) {
            text[extended.len()..].to_vec()
        } else {
            text
        }
    }
    fn same_image(a: &Path, b: &Path) -> bool {
        // SAFETY: borrowed slices have explicit lengths; comparison performs no I/O.
        unsafe { CompareStringOrdinal(&wide_key(a), &wide_key(b), true) == CSTR_EQUAL }
    }
    fn resolved(path: &Path) -> io::Result<PathBuf> {
        crate::safe_path::no_links(path)?;
        if path.file_name().is_none() {
            return Err(io::Error::other("An exact executable path is required."));
        }
        let mut ancestor = path.to_path_buf();
        let mut suffix = Vec::new();
        loop {
            match fs::canonicalize(&ancestor) {
                Ok(mut canonical) => {
                    for name in suffix.iter().rev() {
                        canonical.push(name);
                    }
                    crate::safe_path::no_links(&canonical)?;
                    return Ok(canonical);
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    suffix.push(
                        ancestor
                            .file_name()
                            .ok_or_else(|| {
                                io::Error::other("Executable path has no existing ancestor.")
                            })?
                            .to_owned(),
                    );
                    ancestor.pop();
                }
                Err(error) => return Err(error),
            }
        }
    }
    struct Target {
        identity: Identity,
        handle: Handle,
        image: Option<ImageFile>,
        allowed_scope: bool,
        inspection_error: Option<String>,
    }
    struct Owner {
        sid: Vec<u8>,
        session: u32,
        pid: u32,
    }
    impl Owner {
        fn current() -> io::Result<Self> {
            // SAFETY: pseudo process handle is borrowed, never closed.
            let pid = unsafe { GetCurrentProcessId() };
            Ok(Self {
                sid: user_sid(unsafe { GetCurrentProcess() })?,
                session: session_id(pid)?,
                pid,
            })
        }
        fn permits(&self, pid: u32, process: HANDLE) -> io::Result<bool> {
            Ok(pid != self.pid
                && session_id(pid)? == self.session
                && user_sid(process)? == self.sid)
        }
    }
    fn collect(app: &Application, owner: &Owner) -> io::Result<Vec<Target>> {
        // Basename is only an enumeration prefilter, never shutdown authority.
        let name: Vec<u16> = app
            .path
            .file_name()
            .ok_or_else(|| io::Error::other("No executable filename."))?
            .encode_wide()
            .collect();
        let snapshot = Handle(
            unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }
                .map_err(|e| api_error("CreateToolhelp32Snapshot", e))?,
        );
        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut next = unsafe { Process32FirstW(snapshot.0, &mut entry) };
        let mut targets = Vec::new();
        let mut visited = 0usize;
        loop {
            match next {
                Err(ref error) if win_code(error) == ERROR_NO_MORE_FILES.0 => break,
                Err(error) => return Err(api_error("Enumerate processes", error)),
                Ok(()) => {}
            }
            visited += 1;
            if visited > 65536 {
                return Err(io::Error::other(
                    "Process snapshot exceeds its bounded limit.",
                ));
            }
            let count = entry
                .szExeFile
                .iter()
                .position(|ch| *ch == 0)
                .unwrap_or(entry.szExeFile.len());
            if unsafe { CompareStringOrdinal(&entry.szExeFile[..count], &name, true) } == CSTR_EQUAL
            {
                let pid = entry.th32ProcessID;
                match unsafe {
                    OpenProcess(
                        PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                        false,
                        pid,
                    )
                } {
                    Err(error) if win_code(&error) == ERROR_INVALID_PARAMETER.0 => {}
                    Err(error) => {
                        return Err(api_error(
                            &format!("Cannot inspect candidate process pid={pid}"),
                            error,
                        ))
                    }
                    Ok(raw) => {
                        let handle = Handle(raw);
                        if alive(handle.0)? {
                            let reported = match image(handle.0) {
                                Ok(path) => Some(path),
                                Err(_) if !alive(handle.0)? => None,
                                Err(error) => return Err(error),
                            };
                            if let Some(reported) = reported {
                                // Only a prefilter: case-insensitive strings are
                                // never enough authority to close a process.
                                let prefiltered = same_image(&app.path, &reported)
                                    || resolved(&reported)
                                        .is_ok_and(|path| same_image(&app.path, &path));
                                if prefiltered {
                                    // Open the ORIGINAL reported spelling, not
                                    // a canonicalized substitute. Retain it with
                                    // the process handle until shutdown ends.
                                    let opened = ImageFile::open(&reported);
                                    let expected = app.identity()?;
                                    let different_file = opened.as_ref().is_ok_and(|image| {
                                        expected.is_some() && !same_file(expected, image.identity)
                                    });
                                    if !different_file {
                                        let identity = Identity {
                                            pid,
                                            created: created(handle.0)?,
                                        };
                                        let (image, image_error) = match opened {
                                        Ok(image) if same_file(expected, image.identity) => (Some(image), None),
                                        Ok(_) => (None, Some("The target executable is missing or its file identity is unavailable.".into())),
                                        Err(error) => (None, Some(error.to_string())),
                                    };
                                        let (same_owner, owner_error) =
                                            match owner.permits(pid, handle.0) {
                                                Ok(value) => (value, None),
                                                Err(error) => (false, Some(error.to_string())),
                                            };
                                        let allowed_scope = same_owner && image.is_some();
                                        targets.push(Target {
                                            identity,
                                            handle,
                                            image,
                                            allowed_scope,
                                            inspection_error: image_error.or(owner_error),
                                        });
                                        if targets.len() > MAX_TARGETS {
                                            return Err(io::Error::other(
                                                "Too many matching application instances.",
                                            ));
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            next = unsafe { Process32NextW(snapshot.0, &mut entry) };
        }
        Ok(targets)
    }
    fn revalidate(
        targets: &[Target],
        app: &Application,
        owner: &Owner,
    ) -> io::Result<Vec<Identity>> {
        let mut live = Vec::new();
        for target in targets {
            if !alive(target.handle.0)? {
                continue;
            }
            if unsafe { GetProcessId(target.handle.0) } != target.identity.pid
                || created(target.handle.0)? != target.identity.created
                || !same_image(&app.path, &resolved(&image(target.handle.0)?)?)
                || !owner.permits(target.identity.pid, target.handle.0)?
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "Application identity/owner/session changed before graceful close.",
                ));
            }
            let original_reported = image(target.handle.0)?;
            let current_image = ImageFile::open(&original_reported)?;
            let retained = target
                .image
                .as_ref()
                .ok_or_else(|| io::Error::other("No retained process image identity."))?;
            if ImageFile::read(retained.handle.0)? != retained.identity
                || !same_file(app.identity()?, retained.identity)
                || current_image.identity != retained.identity
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "Exact executable file identity changed before graceful close.",
                ));
            }
            live.push(target.identity);
        }
        Ok(live)
    }
    struct Session(Option<u32>);
    impl Session {
        fn start(identities: &[Identity]) -> io::Result<Self> {
            let mut key = [0u16; CCH_RM_SESSION_KEY as usize + 1];
            let mut id = 0;
            let status = unsafe { RmStartSession(&mut id, Some(0), PWSTR(key.as_mut_ptr())) };
            if status != ERROR_SUCCESS {
                return Err(io::Error::other(format!(
                    "RmStartSession Win32={}",
                    status.0
                )));
            }
            let session = Self(Some(id));
            let processes: Vec<_> = identities
                .iter()
                .map(|identity| RM_UNIQUE_PROCESS {
                    dwProcessId: identity.pid,
                    ProcessStartTime: FILETIME {
                        dwLowDateTime: identity.created as u32,
                        dwHighDateTime: (identity.created >> 32) as u32,
                    },
                })
                .collect();
            // Never register a file, service or an unverified process name.
            let status = unsafe { RmRegisterResources(id, None, Some(&processes), None) };
            if status != ERROR_SUCCESS {
                return Err(io::Error::other(format!(
                    "RmRegisterResources(processes only) Win32={}",
                    status.0
                )));
            }
            Ok(session)
        }
        fn affected(&self, expected: &[Identity], owner: &Owner) -> io::Result<Vec<Identity>> {
            let mut buffer: Vec<RM_PROCESS_INFO> = Vec::new();
            for _ in 0..4 {
                let (mut needed, mut count, mut reboot) = (0, buffer.len() as u32, 0);
                let pointer = (!buffer.is_empty()).then_some(buffer.as_mut_ptr());
                let status = unsafe {
                    RmGetList(
                        self.0.unwrap(),
                        &mut needed,
                        &mut count,
                        pointer,
                        &mut reboot,
                    )
                };
                if status == ERROR_MORE_DATA {
                    if needed == 0 || needed as usize > MAX_TARGETS {
                        return Err(io::Error::other(
                            "Restart Manager affected set exceeds the requested scope.",
                        ));
                    }
                    buffer.resize(needed as usize, RM_PROCESS_INFO::default());
                    continue;
                }
                if status != ERROR_SUCCESS {
                    return Err(io::Error::other(format!("RmGetList Win32={}", status.0)));
                }
                if reboot != 0 || count as usize > buffer.len() {
                    return Err(io::Error::other(format!(
                        "Restart Manager cannot safely close this set, rebootReasons={reboot}"
                    )));
                }
                buffer.truncate(count as usize);
                let affected: Vec<_> = buffer
                    .iter()
                    .map(|info| {
                        (
                            Identity {
                                pid: info.Process.dwProcessId,
                                created: u64::from(info.Process.ProcessStartTime.dwLowDateTime)
                                    | (u64::from(info.Process.ProcessStartTime.dwHighDateTime)
                                        << 32),
                            },
                            info.TSSessionId == owner.session
                                && info.ApplicationType != RmService
                                && info.ApplicationType != RmCritical
                                && info.strServiceShortName[0] == 0,
                        )
                    })
                    .collect();
                check_affected(expected, &affected)?;
                return Ok(affected.into_iter().map(|(identity, _)| identity).collect());
            }
            Err(io::Error::other(
                "Restart Manager affected set did not stabilize.",
            ))
        }
        fn end(&mut self) -> io::Result<()> {
            if let Some(id) = self.0.take() {
                let status = unsafe { RmEndSession(id) };
                if status != ERROR_SUCCESS {
                    return Err(io::Error::other(format!("RmEndSession Win32={}", status.0)));
                }
            }
            Ok(())
        }
    }
    impl Drop for Session {
        fn drop(&mut self) {
            let _ = self.end();
        }
    }

    pub(super) fn ensure_closed(app: &Path, allow_close: bool) -> io::Result<()> {
        let app = Application::new(app)?;
        let owner = Owner::current()?;
        let targets = collect(&app, &owner)?;
        if allow_close {
            if let Some(error) = targets
                .iter()
                .find_map(|target| target.inspection_error.as_ref())
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!("Cannot verify matching process identity/owner/session: {error}"),
                ));
            }
        }
        if !check_consent(
            &targets
                .iter()
                .map(|target| target.allowed_scope)
                .collect::<Vec<_>>(),
            allow_close,
        )? {
            return Ok(());
        }
        let requested = revalidate(&targets, &app, &owner)?;
        if requested.is_empty() {
            return if collect(&app, &owner)?.is_empty() {
                Ok(())
            } else {
                Err(io::Error::other(
                    "A new application instance appeared before graceful close.",
                ))
            };
        }
        let mut session = Session::start(&requested)?;
        let result = (|| {
            let affected = session.affected(&requested, &owner)?;
            let still_live = revalidate(&targets, &app, &owner)?;
            if still_live
                .iter()
                .any(|identity| !affected.contains(identity))
            {
                return Err(io::Error::other(
                    "Restart Manager did not identify every live requested process.",
                ));
            }
            if !affected.is_empty() {
                // Zero flags: do not force unresponsive processes to terminate.
                // Windows performs its synchronous graceful-shutdown protocol.
                let status = unsafe { RmShutdown(session.0.unwrap(), 0, None) };
                if status != ERROR_SUCCESS {
                    return Err(io::Error::other(format!(
                        "RmShutdown(no force) Win32={}",
                        status.0
                    )));
                }
                session.affected(&requested, &owner)?;
            }
            let deadline = Instant::now() + WAIT_BUDGET;
            for target in &targets {
                let remaining = deadline.saturating_duration_since(Instant::now());
                let millis = remaining.as_millis().min(u128::from(u32::MAX - 1)) as u32;
                match unsafe { WaitForSingleObject(target.handle.0, millis) } {
                    WAIT_OBJECT_0 => {}
                    WAIT_TIMEOUT => return Err(io::Error::new(io::ErrorKind::TimedOut, "Graceful application exit exceeded the bounded wait; no process was forced to stop.")),
                    _ => return Err(io::Error::last_os_error()),
                }
            }
            if !collect(&app, &owner)?.is_empty() {
                return Err(io::Error::other("A new application instance is still running; it was not authorized for shutdown."));
            }
            Ok(())
        })();
        let cleanup = session.end();
        match (result, cleanup) {
            (Err(error), Err(cleanup)) => Err(io::Error::other(format!(
                "{error}; session cleanup: {cleanup}"
            ))),
            (Err(error), _) | (_, Err(error)) => Err(error),
            _ => Ok(()),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn exact_image_comparison_keeps_siblings_other_installs_and_native_unicode_distinct() {
            assert!(same_image(
                Path::new(r"\\?\C:\한글\Prism.exe"),
                Path::new(r"c:\한글\prism.EXE")
            ));
            assert!(same_image(
                Path::new(r"\\?\UNC\server\share\Prism.exe"),
                Path::new(r"\\SERVER\share\prism.exe")
            ));
            for other in [
                r"C:\한글-other\prism.exe",
                r"C:\elsewhere\prism.exe",
                r"C:\한글\prism-helper.exe",
            ] {
                assert!(!same_image(
                    Path::new(r"C:\한글\prism.exe"),
                    Path::new(other)
                ));
            }
        }

        #[test]
        fn retained_native_file_identity_detects_replacement_without_launching_any_app() {
            let root = tempfile::tempdir().unwrap();
            let app = root.path().join("prism.exe");
            let old = root.path().join("old.exe");
            fs::write(&app, b"owned metadata fixture A").unwrap();
            let retained = ImageFile::open(&app).unwrap();
            fs::rename(&app, &old).unwrap();
            fs::write(&app, b"owned metadata fixture B").unwrap();
            let replacement = ImageFile::open(&app).unwrap();
            assert_eq!(
                ImageFile::read(retained.handle.0).unwrap(),
                retained.identity
            );
            assert!(!same_file(Some(retained.identity), replacement.identity));
            let absent = Application::new(&root.path().join("missing.exe")).unwrap();
            assert!(!same_file(absent.identity().unwrap(), replacement.identity));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn no_processes_need_no_consent_or_restart_manager_and_default_never_closes() {
        assert!(!check_consent(&[], false).unwrap());
        assert!(!check_consent(&[], true).unwrap());
        assert!(check_consent(&[true], false)
            .unwrap_err()
            .to_string()
            .starts_with("APP_RUNNING:"));
        assert!(check_consent(&[false], false)
            .unwrap_err()
            .to_string()
            .starts_with("APP_RUNNING:"));
        assert!(check_consent(&[true, true], true).unwrap());
        assert!(check_consent(&[true, false], true)
            .unwrap_err()
            .to_string()
            .starts_with("APP_CLOSE_FAILED:"));
    }
    #[test]
    fn affected_scope_rejects_pid_reuse_other_processes_foreign_sessions_and_duplicates() {
        let original = Identity {
            pid: 17,
            created: 100,
        };
        assert!(check_affected(&[original], &[]).is_ok());
        assert!(check_affected(&[original], &[(original, true)]).is_ok());
        for bad in [
            Identity {
                pid: 17,
                created: 101,
            },
            Identity {
                pid: 18,
                created: 100,
            },
        ] {
            assert!(check_affected(&[original], &[(bad, true)]).is_err());
        }
        assert!(check_affected(&[original], &[(original, false)]).is_err());
        assert!(check_affected(&[original], &[(original, true), (original, true)]).is_err());
    }
    #[test]
    fn stable_error_codes_keep_underlying_technical_details() {
        let error = close_failed(io::Error::other("RmShutdown(no force) Win32=351"));
        assert_eq!(
            error.to_string(),
            "APP_CLOSE_FAILED: RmShutdown(no force) Win32=351"
        );
        let running = check_consent(&[true], false).unwrap_err();
        assert_eq!(
            close_failed(io::Error::other(running.to_string())).to_string(),
            running.to_string()
        );
    }

    #[test]
    fn case_folded_aliases_and_missing_targets_cannot_authorize_different_file_identities() {
        let intended = FileIdentity {
            volume: 8,
            index: [41; 16],
        };
        let differently_cased_file = FileIdentity {
            volume: 8,
            index: [42; 16],
        };
        let other_volume = FileIdentity {
            volume: 8 + (1 << 32),
            index: [41; 16],
        };
        assert!(same_file(Some(intended), intended));
        assert!(!same_file(Some(intended), differently_cased_file));
        assert!(!same_file(Some(intended), other_volume));
        let mut same_low_64_bits = intended;
        same_low_64_bits.index[15] ^= 1;
        assert!(!same_file(Some(intended), same_low_64_bits));
        assert!(!same_file(None, intended));
        assert!(!same_file(
            Some(FileIdentity {
                volume: 8,
                index: [0; 16]
            }),
            FileIdentity {
                volume: 8,
                index: [0; 16]
            }
        ));
    }
}

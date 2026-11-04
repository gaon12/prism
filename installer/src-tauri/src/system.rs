//! The real Windows side of [`crate::install::System`]: known folders, `.lnk` shortcuts and the
//! "Apps & features" registry entry. Everything is per-user (HKCU, the user's own folders), so
//! the setup never needs administrator rights.
//!
//! On other operating systems the setup is not used (macOS and Linux ship .dmg/AppImage), so a
//! stub that returns an error keeps `cargo test` working there.

use crate::install::{Folders, System, UninstallEntry};
use std::{
    io,
    path::{Path, PathBuf},
};

/// Registry key under HKCU. Same name the NSIS installer used, so we take over its entry.
pub const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Prism";

pub struct WindowsSystem;

/// Environment variable that switches the setup into a test sandbox (see [`SandboxSystem`]).
pub const SANDBOX_ENV: &str = "PRISM_SETUP_SANDBOX";

/// The system the setup should use: the real one, or the sandbox when `PRISM_SETUP_SANDBOX`
/// names a folder. Child processes (the temporary uninstaller copy) inherit the variable, so a
/// sandboxed install is also uninstalled from the sandbox.
pub fn current() -> Box<dyn System + Send> {
    match std::env::var_os(SANDBOX_ENV) {
        Some(root) => Box::new(SandboxSystem {
            root: PathBuf::from(root),
        }),
        _ => Box::new(WindowsSystem),
    }
}

/// Test mode for trying the real setup program without touching the computer's real state:
/// - "known folders" (Desktop, Start menu, AppData) are sub-folders of the sandbox folder,
/// - the "Apps & features" entry is a JSON file in the sandbox instead of the registry.
///
/// Shortcuts are still real `.lnk` files (inside the sandbox), so that code path is tested too.
/// Nothing outside the sandbox is written, except the temporary uninstaller copy in %TEMP%,
/// which deletes itself.
pub struct SandboxSystem {
    pub root: PathBuf,
}

impl SandboxSystem {
    pub fn checked_root(&self) -> io::Result<PathBuf> {
        if !self.root.is_absolute() || self.root.parent().is_none() {
            return Err(io::Error::other(
                "Sandbox mode requires a full private folder path.",
            ));
        }
        crate::safe_path::no_links(&self.root)?;
        std::fs::create_dir_all(&self.root)?;
        crate::safe_path::root(&self.root)
    }
    fn entry_file(&self) -> io::Result<PathBuf> {
        let root = self.checked_root()?;
        crate::safe_path::file(&root, "uninstall-entry.json")
    }
}

pub fn sandbox_root() -> io::Result<Option<PathBuf>> {
    match std::env::var_os(SANDBOX_ENV) {
        Some(root) => SandboxSystem { root: root.into() }.checked_root().map(Some),
        None => Ok(None),
    }
}

/// Sandbox mode must never accept a frontend/record/CLI path outside its own root.
pub fn check_sandbox_destination(path: &Path) -> io::Result<()> {
    let Some(root) = sandbox_root()? else {
        return Ok(());
    };
    crate::safe_path::no_links(path)?;
    let mut ancestor = path.to_path_buf();
    let mut missing = Vec::new();
    while !ancestor.exists() {
        missing.push(
            ancestor
                .file_name()
                .ok_or_else(|| io::Error::other("Invalid sandbox destination."))?
                .to_owned(),
        );
        ancestor = ancestor
            .parent()
            .ok_or_else(|| io::Error::other("Invalid sandbox destination."))?
            .to_path_buf();
    }
    let mut resolved = std::fs::canonicalize(ancestor)?;
    for name in missing.into_iter().rev() {
        resolved.push(name);
    }
    if !resolved.starts_with(root) {
        return Err(io::Error::other(
            "Sandbox destination is outside its private folder.",
        ));
    }
    Ok(())
}

impl System for SandboxSystem {
    fn shortcut_points_to(&self, link: &Path, target: &Path) -> io::Result<bool> {
        WindowsSystem.shortcut_points_to(link, target)
    }
    fn capture_registration(&self) -> io::Result<Vec<u8>> {
        crate::safe_path::no_links(&self.entry_file()?)?;
        let bytes = match std::fs::read(self.entry_file()?) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        Ok(serde_json::to_vec(&bytes)?)
    }
    fn restore_registration(&self, snapshot: &[u8]) -> io::Result<()> {
        crate::safe_path::no_links(&self.entry_file()?)?;
        let previous: Option<Vec<u8>> = serde_json::from_slice(snapshot)?;
        match previous {
            Some(bytes) => std::fs::write(self.entry_file()?, bytes),
            None => self.remove_uninstall_entry(),
        }
    }
    fn folders(&self) -> io::Result<Folders> {
        let root = self.checked_root()?;
        let folders = Folders {
            local_app_data: root.join("LocalAppData"),
            app_data: root.join("RoamingAppData"),
            desktop: root.join("Desktop"),
            start_menu: root.join("StartMenu"),
        };
        for dir in [
            &folders.local_app_data,
            &folders.app_data,
            &folders.desktop,
            &folders.start_menu,
        ] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(folders)
    }

    fn create_shortcut(&self, link: &Path, target: &Path, description: &str) -> io::Result<()> {
        if !link.starts_with(self.checked_root()?) {
            return Err(io::Error::other(
                "Sandbox: shortcut outside the sandbox refused.",
            ));
        }
        WindowsSystem.create_shortcut(link, target, description)
    }

    fn write_uninstall_entry(&self, entry: &UninstallEntry) -> io::Result<()> {
        let json = serde_json::json!({
            "displayVersion": entry.version,
            "installLocation": entry.install_dir,
            "estimatedSizeKib": entry.size_kib,
        });
        std::fs::write(self.entry_file()?, serde_json::to_vec_pretty(&json)?)
    }

    fn remove_uninstall_entry(&self) -> io::Result<()> {
        match std::fs::remove_file(self.entry_file()?) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }

    fn existing_install(&self) -> Option<(PathBuf, Option<String>)> {
        let bytes = std::fs::read(self.entry_file().ok()?).ok()?;
        let json: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
        let dir = PathBuf::from(json["installLocation"].as_str()?);
        let version = json["displayVersion"].as_str().map(str::to_owned);
        Some((dir, version))
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use crate::install::{APP_EXE, UNINSTALLER_EXE};
    use windows::{
        core::{Interface, GUID, HSTRING, PWSTR},
        Win32::{
            Storage::FileSystem::GetDiskFreeSpaceExW,
            System::Com::{
                CoCreateInstance, CoInitializeEx, CoTaskMemFree, IPersistFile,
                CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
            },
            UI::Shell::{
                FOLDERID_Desktop, FOLDERID_LocalAppData, FOLDERID_Programs,
                FOLDERID_RoamingAppData, IShellLinkW, SHGetKnownFolderPath, ShellLink,
                KNOWN_FOLDER_FLAG,
            },
        },
    };
    use winreg::{enums::HKEY_CURRENT_USER, RegKey};

    fn to_io(error: windows::core::Error) -> io::Error {
        io::Error::other(error.message())
    }

    // Filesystem validation uses canonical extended paths, but ShellLink rejects that
    // spelling for these parameters. Convert only at the shell API boundary.
    fn shell_path(path: &Path) -> PathBuf {
        let value = path.to_string_lossy();
        if let Some(rest) = value.strip_prefix(r"\\?\UNC\") {
            PathBuf::from(format!(r"\\{rest}"))
        } else if let Some(rest) = value.strip_prefix(r"\\?\") {
            PathBuf::from(rest)
        } else {
            path.to_path_buf()
        }
    }

    /// Asks Windows where a "known folder" (Desktop, Start menu, ...) is for this user.
    /// This respects folders the user moved, e.g. a Desktop redirected to OneDrive.
    fn known_folder(id: &GUID) -> io::Result<PathBuf> {
        // SAFETY: Windows allocates the returned string; we copy it and free it right away.
        unsafe {
            let raw: PWSTR = SHGetKnownFolderPath(id, KNOWN_FOLDER_FLAG(0), None).map_err(to_io)?;
            let path = raw
                .to_string()
                .map_err(|_| io::Error::other("Bad folder path."));
            CoTaskMemFree(Some(raw.0 as *const _));
            path.map(PathBuf::from)
        }
    }

    impl System for WindowsSystem {
        fn shortcut_points_to(&self, link: &Path, target: &Path) -> io::Result<bool> {
            use windows::Win32::System::Com::STGM_READ;
            use windows::Win32::UI::Shell::SLGP_RAWPATH;
            // Read only the link target; do not resolve it or launch its target application.
            unsafe {
                let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
                let shell_link: IShellLinkW =
                    CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).map_err(to_io)?;
                let file: IPersistFile = shell_link.cast().map_err(to_io)?;
                file.Load(&HSTRING::from(shell_path(link).as_os_str()), STGM_READ)
                    .map_err(to_io)?;
                let mut buffer = vec![0u16; 32768];
                shell_link
                    .GetPath(&mut buffer, std::ptr::null_mut(), SLGP_RAWPATH.0 as u32)
                    .map_err(to_io)?;
                let end = buffer
                    .iter()
                    .position(|value| *value == 0)
                    .unwrap_or(buffer.len());
                let value = String::from_utf16(&buffer[..end])
                    .map_err(|_| io::Error::other("Invalid shortcut target."))?;
                Ok(crate::safe_path::same_target(Path::new(&value), target))
            }
        }
        fn capture_registration(&self) -> io::Result<Vec<u8>> {
            let key = match RegKey::predef(HKEY_CURRENT_USER).open_subkey(UNINSTALL_KEY) {
                Ok(key) => key,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    return Ok(serde_json::to_vec(
                        &Option::<Vec<(String, u32, Vec<u8>)>>::None,
                    )?)
                }
                Err(error) => return Err(error),
            };
            // Refuse an unfamiliar nested registration rather than promise a partial backup.
            if key.enum_keys().next().is_some() {
                return Err(io::Error::other(
                    "Nested uninstall registration is unsupported.",
                ));
            }
            let mut values = Vec::new();
            for value in key.enum_values() {
                let (name, value) = value?;
                values.push((name, value.vtype as u32, value.bytes));
            }
            Ok(serde_json::to_vec(&Some(values))?)
        }
        fn restore_registration(&self, snapshot: &[u8]) -> io::Result<()> {
            use winreg::enums::*;
            let previous: Option<Vec<(String, u32, Vec<u8>)>> = serde_json::from_slice(snapshot)?;
            let Some(previous) = previous else {
                return self.remove_uninstall_entry();
            };
            let mut values = Vec::new();
            for (name, kind, bytes) in previous {
                let vtype = match kind {
                    0 => REG_NONE,
                    1 => REG_SZ,
                    2 => REG_EXPAND_SZ,
                    3 => REG_BINARY,
                    4 => REG_DWORD,
                    5 => REG_DWORD_BIG_ENDIAN,
                    6 => REG_LINK,
                    7 => REG_MULTI_SZ,
                    8 => REG_RESOURCE_LIST,
                    9 => REG_FULL_RESOURCE_DESCRIPTOR,
                    10 => REG_RESOURCE_REQUIREMENTS_LIST,
                    11 => REG_QWORD,
                    _ => return Err(io::Error::other("Unsupported registry value type.")),
                };
                values.push((
                    name,
                    winreg::RegValue {
                        bytes: bytes.into(),
                        vtype,
                    },
                ));
            }
            let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(UNINSTALL_KEY)?;
            let current: Vec<String> = key
                .enum_values()
                .map(|value| value.map(|(name, _)| name))
                .collect::<io::Result<_>>()?;
            for name in current {
                if !values.iter().any(|(old, _)| *old == name) {
                    key.delete_value(name)?;
                }
            }
            for (name, value) in values {
                key.set_raw_value(name, &value)?;
            }
            Ok(())
        }
        fn folders(&self) -> io::Result<Folders> {
            Ok(Folders {
                local_app_data: known_folder(&FOLDERID_LocalAppData)?,
                app_data: known_folder(&FOLDERID_RoamingAppData)?,
                desktop: known_folder(&FOLDERID_Desktop)?,
                start_menu: known_folder(&FOLDERID_Programs)?,
            })
        }

        fn create_shortcut(&self, link: &Path, target: &Path, description: &str) -> io::Result<()> {
            // SAFETY: plain COM calls on objects we create and release in this function.
            unsafe {
                // Ignore "already initialised"; any other failure shows up in CoCreateInstance.
                let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
                let shell_link: IShellLinkW =
                    CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).map_err(to_io)?;
                shell_link
                    .SetPath(&HSTRING::from(shell_path(target).as_os_str()))
                    .map_err(to_io)?;
                if let Some(folder) = target.parent() {
                    shell_link
                        .SetWorkingDirectory(&HSTRING::from(shell_path(folder).as_os_str()))
                        .map_err(to_io)?;
                }
                shell_link
                    .SetDescription(&HSTRING::from(description))
                    .map_err(to_io)?;
                shell_link
                    .SetIconLocation(&HSTRING::from(shell_path(target).as_os_str()), 0)
                    .map_err(to_io)?;
                let file: IPersistFile = shell_link.cast().map_err(to_io)?;
                file.Save(&HSTRING::from(shell_path(link).as_os_str()), true)
                    .map_err(to_io)
            }
        }

        fn write_uninstall_entry(&self, entry: &UninstallEntry) -> io::Result<()> {
            let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(UNINSTALL_KEY)?;
            let dir = entry.install_dir.display().to_string();
            let uninstaller = entry
                .install_dir
                .join(UNINSTALLER_EXE)
                .display()
                .to_string();
            key.set_value("DisplayName", &"Prism")?;
            key.set_value("DisplayVersion", &entry.version)?;
            key.set_value("Publisher", &"Prism")?;
            key.set_value(
                "DisplayIcon",
                &entry.install_dir.join(APP_EXE).display().to_string(),
            )?;
            key.set_value("InstallLocation", &dir)?;
            key.set_value("UninstallString", &format!("\"{uninstaller}\""))?;
            key.set_value("QuietUninstallString", &format!("\"{uninstaller}\" /S"))?;
            key.set_value("NoModify", &1u32)?;
            key.set_value("NoRepair", &1u32)?;
            key.set_value("EstimatedSize", &entry.size_kib)?;
            Ok(())
        }

        fn remove_uninstall_entry(&self) -> io::Result<()> {
            match RegKey::predef(HKEY_CURRENT_USER).delete_subkey_all(UNINSTALL_KEY) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
                _ => Ok(()),
            }
        }

        fn existing_install(&self) -> Option<(PathBuf, Option<String>)> {
            let key = RegKey::predef(HKEY_CURRENT_USER)
                .open_subkey(UNINSTALL_KEY)
                .ok()?;
            // NSIS wrote the folder with quotes in some versions; strip them.
            let dir: String = key.get_value("InstallLocation").ok()?;
            let dir = PathBuf::from(dir.trim_matches('"'));
            let version = key.get_value("DisplayVersion").ok();
            Some((dir, version))
        }
    }

    /// Free bytes on the drive that would hold `dir` (walks up to a folder that exists).
    pub fn free_bytes(dir: &Path) -> Option<u64> {
        let existing = dir.ancestors().find(|folder| folder.exists())?;
        let mut free = 0u64;
        // SAFETY: `free` outlives the call; the other outputs are not requested.
        unsafe {
            GetDiskFreeSpaceExW(
                &HSTRING::from(existing.as_os_str()),
                Some(&mut free),
                None,
                None,
            )
            .ok()?;
        }
        Some(free)
    }

    /// The setup window needs Microsoft Edge WebView2 (built into Windows 11 and updated
    /// Windows 10). Its installed version is recorded under one of these keys.
    pub fn webview2_installed() -> bool {
        use winreg::enums::HKEY_LOCAL_MACHINE;
        const CLIENT: &str = r"Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";
        let places = [
            (
                HKEY_LOCAL_MACHINE,
                format!(r"SOFTWARE\WOW6432Node\{CLIENT}"),
            ),
            (HKEY_LOCAL_MACHINE, format!(r"SOFTWARE\{CLIENT}")),
            (HKEY_CURRENT_USER, format!(r"Software\{CLIENT}")),
        ];
        places.iter().any(|(root, path)| {
            RegKey::predef(*root)
                .open_subkey(path)
                .and_then(|key| key.get_value::<String, _>("pv"))
                .is_ok_and(|version| !version.is_empty() && version != "0.0.0.0")
        })
    }
}

#[cfg(windows)]
pub use imp::{free_bytes, webview2_installed};

#[cfg(not(windows))]
mod imp {
    use super::*;
    fn unsupported() -> io::Error {
        io::Error::other("The Prism setup only runs on Windows.")
    }
    impl System for WindowsSystem {
        fn shortcut_points_to(&self, _: &Path, _: &Path) -> io::Result<bool> {
            Err(unsupported())
        }
        fn capture_registration(&self) -> io::Result<Vec<u8>> {
            Err(unsupported())
        }
        fn restore_registration(&self, _: &[u8]) -> io::Result<()> {
            Err(unsupported())
        }
        fn folders(&self) -> io::Result<Folders> {
            Err(unsupported())
        }
        fn create_shortcut(&self, _: &Path, _: &Path, _: &str) -> io::Result<()> {
            Err(unsupported())
        }
        fn write_uninstall_entry(&self, _: &UninstallEntry) -> io::Result<()> {
            Err(unsupported())
        }
        fn remove_uninstall_entry(&self) -> io::Result<()> {
            Err(unsupported())
        }
        fn existing_install(&self) -> Option<(PathBuf, Option<String>)> {
            None
        }
    }
    pub fn free_bytes(_: &Path) -> Option<u64> {
        None
    }
    pub fn webview2_installed() -> bool {
        true
    }
}

#[cfg(not(windows))]
pub use imp::{free_bytes, webview2_installed};

#[cfg(test)]
mod sandbox_tests {
    use super::*;
    #[test]
    fn empty_and_relative_sandbox_roots_fail_instead_of_using_windows() {
        for root in [PathBuf::new(), PathBuf::from("relative-sandbox")] {
            let sandbox = SandboxSystem { root };
            assert!(sandbox.folders().is_err());
            assert!(sandbox.capture_registration().is_err());
            assert!(sandbox.remove_uninstall_entry().is_err());
        }
    }
    #[test]
    #[cfg(windows)]
    fn windows_shell_links_roundtrip_only_inside_a_temporary_sandbox() {
        let temporary = tempfile::tempdir().unwrap();
        let sandbox = SandboxSystem {
            root: temporary.path().into(),
        };
        let folders = sandbox.folders().unwrap();
        let target = folders.local_app_data.join("prism.exe");
        std::fs::write(&target, b"fixture target, never executed").unwrap();
        let link = crate::safe_path::file(&folders.desktop, "Prism.lnk").unwrap();
        sandbox.create_shortcut(&link, &target, "Fixture").unwrap();
        assert!(sandbox.shortcut_points_to(&link, &target).unwrap());
        assert!(!sandbox
            .shortcut_points_to(&link, &folders.local_app_data.join("other.exe"))
            .unwrap());
    }
}

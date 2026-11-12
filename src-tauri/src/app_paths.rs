//! Data locations and OS integration policy for each distribution format.
//!
//! A portable copy is marked by a `prism.portable` file next to the executable. All of its data
//! (settings, session, clipboard history, dictionaries, indexes and the WebView2 profile) lives in
//! `PrismData` beside the executable, and it never silently falls back to the user profile:
//! Prism's managed data stays with the portable copy.

use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

pub const PORTABLE_MARKER: &str = "prism.portable";
pub const PORTABLE_DATA: &str = "PrismData";

pub fn is_portable(exe: &Path) -> bool {
    exe.parent()
        .is_some_and(|dir| dir.join(PORTABLE_MARKER).is_file())
}

/// A directory owns one data store. Separate portable copies must not hand off
/// to an installed instance or concurrently write the same portable settings.
pub fn portable_identifier(base: &str, exe: &Path) -> String {
    let dir = exe.parent().unwrap_or(exe);
    let path = fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let path = path.to_string_lossy().replace('\\', "/");
    #[cfg(windows)]
    let path = path.to_lowercase();
    let hash = Sha256::digest(path.as_bytes());
    format!("{base}.portable.p{hash:x}")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum InstallKind {
    Installed,
    Portable,
    /// An MSIX package owns its LocalState and is updated by Windows.
    Packaged,
    /// Debug-only isolated test directory (`PRISM_DEV_DATA_DIR`).
    Isolated,
    /// A Linux Flatpak. Flatpak updates the read-only `/app` copy; data stays
    /// in the sandbox's own config folder (`~/.var/app/<id>/config/...`).
    Flatpak,
}

/// Whether this process runs inside a Flatpak sandbox. Flatpak always sets
/// `FLATPAK_ID` and mounts `/.flatpak-info`; either is enough.
pub fn in_flatpak() -> bool {
    cfg!(target_os = "linux")
        && (std::env::var_os("FLATPAK_ID").is_some() || Path::new("/.flatpak-info").is_file())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataLocation {
    pub dir: PathBuf,
    pub kind: InstallKind,
}
impl DataLocation {
    /// WebView2 user data folder. Portable and isolated runs keep it inside their data folder.
    pub fn webview_dir(&self) -> Option<PathBuf> {
        match self.kind {
            InstallKind::Installed | InstallKind::Flatpak => None,
            _ => Some(self.dir.join("WebView2")),
        }
    }
    /// Only ordinary installations use the autostart plugin's absolute registry path.
    /// Packaged copies use a manifest StartupTask, Flatpaks a host autostart entry.
    pub fn allows_autostart(&self) -> bool {
        self.kind == InstallKind::Installed
    }
    /// Portable copies are updated by replacing the folder, Flatpaks by Flatpak,
    /// never by the installer updater.
    pub fn allows_installer_update(&self) -> bool {
        self.kind == InstallKind::Installed
    }
}

/// Resolves where Prism stores data. `installed_dir` is Tauri's `app_config_dir()`.
/// `override_dir` is the debug-only `PRISM_DEV_DATA_DIR`. `flatpak` is
/// [`in_flatpak`]; a Flatpak keeps Tauri's config folder, which Flatpak
/// already redirects into the sandbox.
pub fn resolve(
    exe: &Path,
    installed_dir: PathBuf,
    override_dir: Option<PathBuf>,
    packaged_dir: Option<PathBuf>,
    flatpak: bool,
) -> Result<DataLocation, String> {
    let (dir, kind) = if let Some(dir) = override_dir {
        (dir, InstallKind::Isolated)
    } else if let Some(dir) = packaged_dir {
        (dir, InstallKind::Packaged)
    } else if flatpak {
        (installed_dir, InstallKind::Flatpak)
    } else {
        let exe_dir = exe
            .parent()
            .ok_or("The Prism executable location is unavailable.")?;
        if is_portable(exe) {
            (exe_dir.join(PORTABLE_DATA), InstallKind::Portable)
        } else {
            (installed_dir, InstallKind::Installed)
        }
    };
    fs::create_dir_all(&dir).map_err(|error| {
        format!(
            "Could not create the data folder {}: {error}",
            dir.display()
        )
    })?;
    ensure_writable(&dir)?;
    Ok(DataLocation { dir, kind })
}

fn ensure_writable(dir: &Path) -> Result<(), String> {
    let mut nonce = [0u8; 16];
    getrandom::fill(&mut nonce).map_err(|_| "Cannot validate data folder access.")?;
    let name: String = nonce.iter().map(|byte| format!("{byte:02x}")).collect();
    let probe = dir.join(format!(".write-test-{name}"));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&probe)?;
        file.write_all(b"ok")?;
        drop(file);
        fs::remove_file(&probe)
    })();
    result.map_err(|error: std::io::Error| {
        format!(
            "The data folder {} is not writable: {error}. Move Prism to a writable folder.",
            dir.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installed_portable_and_isolated_locations() {
        let root = tempfile::tempdir().unwrap();
        let app = root.path().join("Prism 앱");
        fs::create_dir(&app).unwrap();
        let exe = app.join("prism.exe");
        let profile = root.path().join("profile");

        let installed = resolve(&exe, profile.clone(), None, None, false).unwrap();
        assert_eq!(installed.kind, InstallKind::Installed);
        assert_eq!(installed.dir, profile);
        assert!(installed.allows_autostart() && installed.webview_dir().is_none());

        fs::write(app.join(PORTABLE_MARKER), "").unwrap();
        let portable = resolve(&exe, profile.clone(), None, None, false).unwrap();
        assert_eq!(portable.kind, InstallKind::Portable);
        assert_eq!(portable.dir, app.join(PORTABLE_DATA));
        assert!(portable.dir.is_dir());
        assert_eq!(
            portable.webview_dir(),
            Some(app.join(PORTABLE_DATA).join("WebView2"))
        );
        assert!(!portable.allows_autostart() && !portable.allows_installer_update());

        let packaged_dir = root.path().join("package/LocalState");
        let packaged = resolve(
            &exe,
            profile.clone(),
            None,
            Some(packaged_dir.clone()),
            false,
        )
        .unwrap();
        assert_eq!(packaged.kind, InstallKind::Packaged);
        assert_eq!(packaged.dir, packaged_dir);
        assert_eq!(packaged.webview_dir(), Some(packaged_dir.join("WebView2")));
        assert!(!packaged.allows_autostart() && !packaged.allows_installer_update());

        let isolated = resolve(
            &exe,
            profile,
            Some(root.path().join("dev")),
            Some(packaged_dir),
            true,
        )
        .unwrap();
        assert_eq!(
            isolated.kind,
            InstallKind::Isolated,
            "isolation wins over Flatpak"
        );
        assert!(isolated.webview_dir().is_some());
        assert_eq!(
            fs::read_dir(&isolated.dir).unwrap().count(),
            0,
            "probe file removed"
        );
    }

    #[test]
    fn a_flatpak_uses_its_sandboxed_config_folder_and_never_self_updates() {
        let root = tempfile::tempdir().unwrap();
        let app = root.path().join("app/bin");
        fs::create_dir_all(&app).unwrap();
        // Even a stray portable marker cannot redirect a Flatpak's data.
        fs::write(app.join(PORTABLE_MARKER), "").unwrap();
        let config = root
            .path()
            .join("var/app/app.prism.desktop/config/app.prism.desktop");
        let flatpak = resolve(&app.join("prism"), config.clone(), None, None, true).unwrap();
        assert_eq!(flatpak.kind, InstallKind::Flatpak);
        assert_eq!(flatpak.dir, config);
        assert!(flatpak.webview_dir().is_none());
        assert!(!flatpak.allows_autostart() && !flatpak.allows_installer_update());
    }

    #[test]
    fn portable_never_falls_back_to_the_profile() {
        let root = tempfile::tempdir().unwrap();
        let exe = root.path().join("prism.exe");
        fs::write(root.path().join(PORTABLE_MARKER), "").unwrap();
        // A file where the data folder should be makes it impossible to create.
        fs::write(root.path().join(PORTABLE_DATA), "").unwrap();
        let profile = root.path().join("profile");
        let error = resolve(&exe, profile.clone(), None, None, false).unwrap_err();
        assert!(error.contains(PORTABLE_DATA));
        assert!(!profile.exists());
        // A marker directory (not a file) does not enable portable mode.
        let other = tempfile::tempdir().unwrap();
        fs::create_dir(other.path().join(PORTABLE_MARKER)).unwrap();
        let location =
            resolve(&other.path().join("prism.exe"), profile, None, None, false).unwrap();
        assert_eq!(location.kind, InstallKind::Installed);
    }

    #[test]
    fn packaged_data_failure_never_falls_back_to_another_profile() {
        let root = tempfile::tempdir().unwrap();
        let blocked = root.path().join("blocked");
        fs::write(&blocked, "not a directory").unwrap();
        let profile = root.path().join("profile");
        assert!(resolve(
            &root.path().join("prism.exe"),
            profile.clone(),
            None,
            Some(blocked),
            false
        )
        .is_err());
        assert!(!profile.exists());
    }

    #[test]
    fn portable_instances_are_scoped_to_their_data_directory() {
        let root = tempfile::tempdir().unwrap();
        let a = root.path().join("a");
        let b = root.path().join("b");
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&b).unwrap();
        let id = portable_identifier("app.prism.desktop", &a.join("prism.exe"));
        assert_ne!(id, "app.prism.desktop");
        assert_eq!(
            id,
            portable_identifier("app.prism.desktop", &a.join("other.exe"))
        );
        assert_ne!(
            id,
            portable_identifier("app.prism.desktop", &b.join("prism.exe"))
        );
    }
}

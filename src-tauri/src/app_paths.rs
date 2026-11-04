//! Data location for installed vs portable builds.
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
    /// Debug-only isolated test directory (`PRISM_DEV_DATA_DIR`).
    Isolated,
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
            InstallKind::Installed => None,
            _ => Some(self.dir.join("WebView2")),
        }
    }
    /// Login autostart writes an absolute exe path to the registry; a portable copy on a
    /// removable drive must not register itself.
    pub fn allows_autostart(&self) -> bool {
        self.kind == InstallKind::Installed
    }
    /// Portable copies are updated by replacing the folder, not by the installer updater.
    pub fn allows_installer_update(&self) -> bool {
        self.kind == InstallKind::Installed
    }
}

/// Resolves where Prism stores data. `installed_dir` is Tauri's `app_config_dir()`.
/// `override_dir` is the debug-only `PRISM_DEV_DATA_DIR`.
pub fn resolve(
    exe: &Path,
    installed_dir: PathBuf,
    override_dir: Option<PathBuf>,
) -> Result<DataLocation, String> {
    let (dir, kind) = if let Some(dir) = override_dir {
        (dir, InstallKind::Isolated)
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

        let installed = resolve(&exe, profile.clone(), None).unwrap();
        assert_eq!(installed.kind, InstallKind::Installed);
        assert_eq!(installed.dir, profile);
        assert!(installed.allows_autostart() && installed.webview_dir().is_none());

        fs::write(app.join(PORTABLE_MARKER), "").unwrap();
        let portable = resolve(&exe, profile.clone(), None).unwrap();
        assert_eq!(portable.kind, InstallKind::Portable);
        assert_eq!(portable.dir, app.join(PORTABLE_DATA));
        assert!(portable.dir.is_dir());
        assert_eq!(
            portable.webview_dir(),
            Some(app.join(PORTABLE_DATA).join("WebView2"))
        );
        assert!(!portable.allows_autostart() && !portable.allows_installer_update());

        let isolated = resolve(&exe, profile, Some(root.path().join("dev"))).unwrap();
        assert_eq!(isolated.kind, InstallKind::Isolated);
        assert!(isolated.webview_dir().is_some());
        assert_eq!(
            fs::read_dir(&isolated.dir).unwrap().count(),
            0,
            "probe file removed"
        );
    }

    #[test]
    fn portable_never_falls_back_to_the_profile() {
        let root = tempfile::tempdir().unwrap();
        let exe = root.path().join("prism.exe");
        fs::write(root.path().join(PORTABLE_MARKER), "").unwrap();
        // A file where the data folder should be makes it impossible to create.
        fs::write(root.path().join(PORTABLE_DATA), "").unwrap();
        let profile = root.path().join("profile");
        let error = resolve(&exe, profile.clone(), None).unwrap_err();
        assert!(error.contains(PORTABLE_DATA));
        assert!(!profile.exists());
        // A marker directory (not a file) does not enable portable mode.
        let other = tempfile::tempdir().unwrap();
        fs::create_dir(other.path().join(PORTABLE_MARKER)).unwrap();
        let location = resolve(&other.path().join("prism.exe"), profile, None).unwrap();
        assert_eq!(location.kind, InstallKind::Installed);
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

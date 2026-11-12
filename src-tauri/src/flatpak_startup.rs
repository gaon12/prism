//! Start-on-login for the Flatpak build.
//!
//! Inside the sandbox the autostart plugin would write an entry under the
//! sandbox's own config folder (`~/.var/app/<id>/config/autostart`), which the
//! desktop never reads, and its `Exec` would point at `/app/bin/prism`, which
//! only exists inside the sandbox. The Flatpak is granted host file access
//! (`--filesystem=host`), so this writes the entry into the *host* autostart
//! folder instead, launching through `flatpak run`.
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

/// The application id when `FLATPAK_ID` is missing (it is always set by Flatpak).
const DEFAULT_ID: &str = "app.prism.desktop";

/// The host's autostart folder. Flatpak exports the host's own XDG config
/// folder as `HOST_XDG_CONFIG_HOME` when it differs from `~/.config`.
fn autostart_dir(host_config: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    host_config
        .filter(|path| path.is_absolute())
        .or_else(|| {
            home.filter(|path| path.is_absolute())
                .map(|home| home.join(".config"))
        })
        .map(|config| config.join("autostart"))
}

/// Only the reverse-DNS characters Flatpak allows in an application id.
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// The desktop entry that starts Prism in the background after login.
fn entry(id: &str) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=Prism\nExec=flatpak run --command=prism {id} --background\nX-Flatpak={id}\nX-GNOME-Autostart-enabled=true\nNoDisplay=true\n"
    )
}

/// Write or remove `<dir>/<id>.desktop`. Writing goes through a temporary file
/// and a rename, so a crash never leaves a half-written entry.
fn apply(dir: &Path, id: &str, enabled: bool) -> io::Result<()> {
    let path = dir.join(format!("{id}.desktop"));
    if !enabled {
        return match fs::remove_file(&path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            result => result,
        };
    }
    fs::create_dir_all(dir)?;
    let temporary = dir.join(format!(".{id}.desktop.tmp"));
    let mut file = fs::File::create(&temporary)?;
    file.write_all(entry(id).as_bytes())?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temporary, &path)
}

pub fn set(enabled: bool) -> Result<(), String> {
    let id = std::env::var("FLATPAK_ID")
        .ok()
        .filter(|id| valid_id(id))
        .unwrap_or_else(|| DEFAULT_ID.to_string());
    let dir = autostart_dir(
        std::env::var_os("HOST_XDG_CONFIG_HOME").map(PathBuf::from),
        std::env::var_os("HOME").map(PathBuf::from),
    )
    .ok_or("STARTUP_CHANGE_FAILED")?;
    apply(&dir, &id, enabled).map_err(|_| "STARTUP_CHANGE_FAILED".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_config_wins_and_relative_paths_are_ignored() {
        // Absolute on every OS the tests run on (CI also tests on Windows).
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let host = root.path().join("host-config");
        assert_eq!(
            autostart_dir(Some(host.clone()), Some(home.clone())),
            Some(host.join("autostart"))
        );
        assert_eq!(
            autostart_dir(Some("relative".into()), Some(home.clone())),
            Some(home.join(".config/autostart"))
        );
        assert_eq!(autostart_dir(None, Some("relative".into())), None);
    }

    #[test]
    fn only_flatpak_ids_are_accepted() {
        assert!(valid_id("app.prism.desktop"));
        assert!(!valid_id(""));
        assert!(!valid_id("app prism"));
        assert!(!valid_id("../evil"));
    }

    #[test]
    fn enabling_writes_a_flatpak_launcher_and_disabling_removes_it() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("autostart");
        apply(&dir, DEFAULT_ID, true).unwrap();
        let path = dir.join("app.prism.desktop.desktop");
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("Exec=flatpak run --command=prism app.prism.desktop --background"));
        assert!(text.contains("X-Flatpak=app.prism.desktop"));
        assert_eq!(
            fs::read_dir(&dir).unwrap().count(),
            1,
            "no temporary file left"
        );
        // Enabling twice keeps one entry; disabling twice is not an error.
        apply(&dir, DEFAULT_ID, true).unwrap();
        apply(&dir, DEFAULT_ID, false).unwrap();
        assert!(!path.exists());
        apply(&dir, DEFAULT_ID, false).unwrap();
    }
}

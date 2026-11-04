//! Filesystem boundaries shared by payload extraction and recorded-file removal.
//! Reject reparse points instead of following a junction outside the selected folder.

use std::{
    fs, io,
    path::{Component, Path, PathBuf},
};

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "An installation path is unsafe.",
    )
}

pub fn relative_name(name: &str) -> io::Result<()> {
    if name.is_empty() || name.len() > u16::MAX as usize {
        return Err(invalid());
    }
    for part in name.split('/') {
        if part.is_empty()
            || matches!(part, "." | "..")
            || part.ends_with(['.', ' '])
            || part
                .chars()
                .any(|ch| ch.is_control() || "\\:<>\"|?*".contains(ch))
        {
            return Err(invalid());
        }
        let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
        if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || ["COM", "LPT"].iter().any(|prefix| {
                stem.strip_prefix(prefix).is_some_and(|suffix| {
                    matches!(
                        suffix,
                        "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                    )
                })
            })
        {
            return Err(invalid());
        }
    }
    Ok(())
}

fn reparse(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

/// Check every existing ancestor, including a dangling link itself. Permission errors
/// are failures, not evidence that a path does not exist.
pub fn no_links(path: &Path) -> io::Result<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return Err(invalid());
    }
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) if reparse(&metadata) => return Err(invalid()),
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

pub fn root(path: &Path) -> io::Result<PathBuf> {
    no_links(path)?;
    let canonical = fs::canonicalize(path)?;
    if canonical.parent().is_none() || !canonical.is_dir() {
        return Err(invalid());
    }
    Ok(canonical)
}

/// Returned absolute path is contained in the resolved root. Repeat this immediately
/// before a mutation; do not keep a path from a previous scan as authorization.
pub fn file(root_path: &Path, name: &str) -> io::Result<PathBuf> {
    relative_name(name)?;
    let root = root(root_path)?;
    let target = root.join(name);
    no_links(&target)?;
    match fs::symlink_metadata(&target) {
        Ok(metadata) if !metadata.is_file() => return Err(invalid()),
        Ok(_) => {
            if !fs::canonicalize(&target)?.starts_with(&root) {
                return Err(invalid());
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    Ok(target)
}

pub fn create_parent(root_path: &Path, name: &str) -> io::Result<PathBuf> {
    let target = file(root_path, name)?;
    let parent = target.parent().ok_or_else(invalid)?;
    fs::create_dir_all(parent)?;
    // Recheck newly created parents before opening the file.
    file(root_path, name)
}

/// A shortcut record may name only the expected Prism link in its known folder.
pub fn shortcut(recorded: &Path, known_folder: &Path) -> io::Result<PathBuf> {
    no_links(recorded)?;
    let parent = recorded.parent().ok_or_else(invalid)?;
    let expected = file(known_folder, "Prism.lnk")?;
    let name = recorded.file_name().ok_or_else(invalid)?.to_string_lossy();
    if !name.eq_ignore_ascii_case("Prism.lnk") || fs::canonicalize(parent)? != root(known_folder)? {
        return Err(invalid());
    }
    Ok(expected)
}

/// Compare stored shortcut targets even after the application file was removed.
pub fn same_target(a: &Path, b: &Path) -> bool {
    fn key(path: &Path) -> Option<PathBuf> {
        if !path.is_absolute()
            || path
                .components()
                .any(|part| matches!(part, Component::ParentDir))
        {
            return None;
        }
        let path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        #[cfg(windows)]
        {
            let value = path.to_string_lossy().to_lowercase();
            let value = if let Some(rest) = value.strip_prefix(r"\\?\unc\") {
                format!(r"\\{rest}")
            } else {
                value.strip_prefix(r"\\?\").unwrap_or(&value).to_owned()
            };
            Some(PathBuf::from(value))
        }
        #[cfg(not(windows))]
        {
            Some(path)
        }
    }
    matches!((key(a), key(b)), (Some(a), Some(b)) if a == b)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names_reject_traversal_windows_aliases_and_device_paths() {
        for name in [
            "../outside",
            "/outside",
            "a/../x",
            "a//b",
            "./x",
            "a\\b",
            "C:x",
            "x:stream",
            "a.",
            "a ",
            "NUL.txt",
            "com1",
            "LPT².log",
            "x\0y",
            "a?b",
        ] {
            assert!(relative_name(name).is_err(), "{name:?}");
        }
        for name in ["prism.exe", "resources/한글.txt", "com10.txt"] {
            relative_name(name).unwrap();
        }
    }
    #[test]
    fn shortcut_records_cannot_select_other_files_or_folders() {
        let temp = tempfile::tempdir().unwrap();
        let desktop = temp.path().join("Desktop");
        fs::create_dir(&desktop).unwrap();
        assert!(shortcut(&desktop.join("personal.txt"), &desktop).is_err());
        assert!(shortcut(&temp.path().join("Prism.lnk"), &desktop).is_err());
        assert!(shortcut(&desktop.join("Prism.lnk"), &desktop).is_ok());
    }
    #[test]
    fn existing_directory_cannot_be_a_file_target() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join("resource")).unwrap();
        assert!(file(temp.path(), "resource").is_err());
        assert!(file(temp.path(), "resource/file.txt").is_ok());
    }
}

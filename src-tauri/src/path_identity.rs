//! Resolve filesystem aliases before comparing stored identities. Comparison of
//! a missing path is for cache bookkeeping only; it never authorizes a read.
use std::{
    fs, io,
    path::{Component, Path, PathBuf},
};

fn unsafe_path() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "Path contains a link or reparse point",
    )
}

pub(crate) fn no_links(path: &Path) -> io::Result<()> {
    for part in path.ancestors().filter(|part| !part.as_os_str().is_empty()) {
        let metadata = fs::symlink_metadata(part)?;
        if metadata.file_type().is_symlink() {
            return Err(unsafe_path());
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Err(unsafe_path());
            }
        }
    }
    Ok(())
}

/// A current absolute path, with all aliases expanded and no linked ancestors.
/// Keep Rust's native canonical spelling (including Windows' extended prefix).
pub(crate) fn canonical(path: &Path) -> io::Result<PathBuf> {
    if !path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Expected an absolute path",
        ));
    }
    no_links(path)?;
    let resolved = fs::canonicalize(path)?;
    no_links(path)?;
    no_links(&resolved)?;
    Ok(resolved)
}

pub(crate) fn normal(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let value = path.to_string_lossy();
        if let Some(rest) = value.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = value.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
    }
    path.to_owned()
}

/// Expand a short ancestor even when a file/child folder has disappeared. Never
/// follow a reparse point while doing so. A fully unavailable volume retains its
/// lexical absolute spelling: this does not make that volume a readable source.
pub(crate) fn comparison(path: &Path) -> io::Result<PathBuf> {
    let absolute = std::path::absolute(normal(path))?;
    let mut ancestor = absolute.clone();
    let mut suffix = Vec::new();
    loop {
        match fs::symlink_metadata(&ancestor) {
            Ok(_) => {
                let mut resolved = normal(&canonical(&ancestor)?);
                for part in suffix.iter().rev() {
                    resolved.push(part);
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                // Folding a missing ".." path could hide a linked ancestor.
                // Existing paths are handled above after checking every ancestor.
                if absolute
                    .components()
                    .any(|part| part == Component::ParentDir)
                {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "Ambiguous missing path",
                    ));
                }
                let Some(name) = ancestor.file_name() else {
                    return Ok(normal(&absolute));
                };
                suffix.push(name.to_os_string());
                if !ancestor.pop() {
                    return Ok(normal(&absolute));
                }
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(all(test, windows))]
pub(crate) fn short_alias(path: &Path) -> io::Result<Option<PathBuf>> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    let input: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut buffer = vec![0u16; 32768];
    let length = unsafe {
        windows_sys::Win32::Storage::FileSystem::GetShortPathNameW(
            input.as_ptr(),
            buffer.as_mut_ptr(),
            buffer.len() as u32,
        )
    };
    if length == 0 {
        return Err(io::Error::last_os_error());
    }
    if length as usize >= buffer.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Short path exceeds fixture buffer",
        ));
    }
    buffer.truncate(length as usize);
    let short = PathBuf::from(std::ffi::OsString::from_wide(&buffer));
    if normal(&short).to_string_lossy().to_lowercase()
        == normal(path).to_string_lossy().to_lowercase()
    {
        Ok(None) // Some volumes do not create 8.3 names; never call that evidence.
    } else {
        Ok(Some(short))
    }
}

#[cfg(all(test, windows))]
pub(crate) fn junction(link: &Path, target: &Path) {
    use std::os::windows::process::CommandExt;
    // Both paths are test-owned children of the same freshly-created fixture.
    assert_eq!(link.parent(), target.parent());
    let result = std::process::Command::new("cmd.exe")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .creation_flags(0x08000000)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "Fixture junction creation failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn existing_and_missing_paths_share_the_canonical_ancestor() {
        let directory = tempfile::tempdir().unwrap();
        let folder = directory.path().join("nested");
        fs::create_dir(&folder).unwrap();
        let file = folder.join("source.md");
        fs::write(&file, "fixture").unwrap();
        let expected = normal(&fs::canonicalize(&folder).unwrap()).join("source.md");
        assert_eq!(comparison(&file).unwrap(), expected);
        fs::remove_file(&file).unwrap();
        assert_eq!(comparison(&file).unwrap(), expected);
        assert_eq!(
            comparison(&folder.join("missing/child.md")).unwrap(),
            normal(&fs::canonicalize(&folder).unwrap()).join("missing/child.md")
        );
        assert!(canonical(&file).is_err());
        assert!(canonical(Path::new("relative.md")).is_err());
    }
    #[cfg(windows)]
    #[test]
    fn real_short_alias_expands_before_and_after_a_child_disappears() {
        let directory = tempfile::tempdir().unwrap();
        let folder = directory
            .path()
            .join("A long folder for actual short names");
        fs::create_dir(&folder).unwrap();
        let Some(short) = short_alias(&folder).unwrap() else {
            eprintln!("No 8.3 aliases on this volume; short-alias case not exercised");
            return;
        };
        let long = normal(&fs::canonicalize(&folder).unwrap());
        assert_eq!(comparison(&short).unwrap(), long);
        fs::write(folder.join("source.md"), "fixture").unwrap();
        assert_eq!(
            comparison(&short.join("source.md")).unwrap(),
            long.join("source.md")
        );
        fs::remove_file(folder.join("source.md")).unwrap();
        assert_eq!(
            comparison(&short.join("source.md")).unwrap(),
            long.join("source.md")
        );
    }
    #[cfg(windows)]
    #[test]
    fn windows_reparse_points_never_become_canonical_or_missing_cache_roots() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("target");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("source.md"), "fixture").unwrap();
        let link = directory.path().join("junction");
        junction(&link, &target);
        assert!(canonical(&link.join("source.md")).is_err());
        assert!(comparison(&link.join("missing.md")).is_err());
        assert!(target.join("source.md").exists());
    }
    #[cfg(unix)]
    #[test]
    fn linked_ancestors_are_rejected_even_for_a_missing_child() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("target");
        fs::create_dir(&target).unwrap();
        let link = directory.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(canonical(&link).is_err());
        assert!(comparison(&link.join("missing.md")).is_err());
    }
}

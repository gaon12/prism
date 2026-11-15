use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, RawHandle};
use std::path::Path;

use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    MoveFileExW, ReOpenFile, SetFileAttributesW, FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_TEMPORARY,
    FILE_FLAG_DELETE_ON_CLOSE, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_SHARE_DELETE,
    FILE_SHARE_READ, FILE_SHARE_WRITE, MOVEFILE_REPLACE_EXISTING,
};

use crate::util;

fn to_utf16(s: &Path) -> io::Result<Vec<u16>> {
    // Rust's std::fs handles long paths, but the direct Win32 calls below do not
    // expand them. In particular Lance persists transaction files past MAX_PATH.
    // Normalize to an absolute backslash path before adding the extended prefix.
    let absolute = std::path::absolute(s)?;
    let wide: Vec<u16> = absolute.as_os_str().encode_wide().collect();
    let mut value = if wide.len() >= 260
        && !wide.starts_with(&[92, 92, 63, 92]) // \\?\ namespace already supplied
        && !wide.starts_with(&[92, 92, 46, 92])
    // \\.\ device namespace
    {
        if wide.starts_with(&[92, 92]) {
            "\\\\?\\UNC\\"
                .encode_utf16()
                .chain(wide[2..].iter().copied())
                .collect()
        } else {
            "\\\\?\\"
                .encode_utf16()
                .chain(wide.iter().copied())
                .collect()
        }
    } else {
        // Keep existing short and explicitly namespaced path semantics.
        s.as_os_str().encode_wide().collect::<Vec<_>>()
    };
    value.push(0);
    Ok(value)
}

fn not_supported<T>(msg: &str) -> io::Result<T> {
    Err(io::Error::new(io::ErrorKind::Other, msg))
}

pub fn create_named(
    path: &Path,
    open_options: &mut OpenOptions,
    permissions: Option<&std::fs::Permissions>,
) -> io::Result<File> {
    if permissions.map_or(false, |p| p.readonly()) {
        return not_supported("changing permissions is not supported on this platform");
    }
    open_options
        .create_new(true)
        .read(true)
        .write(true)
        .custom_flags(FILE_ATTRIBUTE_TEMPORARY)
        .open(path)
}

pub fn create(dir: &Path) -> io::Result<File> {
    util::create_helper(
        dir,
        OsStr::new(".tmp"),
        OsStr::new(""),
        crate::NUM_RAND_CHARS,
        |path| {
            let f = OpenOptions::new()
                .create_new(true)
                .read(true)
                .write(true)
                .share_mode(0)
                .custom_flags(FILE_ATTRIBUTE_TEMPORARY | FILE_FLAG_DELETE_ON_CLOSE)
                .open(path)?;
            // NOTE: in theory, we could delete the file immediately (we open the file in "unix
            // semantics" mode) but this seemed to corrupt something in Windows at scale (see #339).
            // So we just rely on `FILE_FLAG_DELETE_ON_CLOSE`.
            Ok(f)
        },
    )
}

pub fn reopen(file: &File, _path: &Path) -> io::Result<File> {
    let handle = file.as_raw_handle();
    unsafe {
        let handle = ReOpenFile(
            handle as HANDLE,
            FILE_GENERIC_READ | FILE_GENERIC_WRITE,
            FILE_SHARE_DELETE | FILE_SHARE_READ | FILE_SHARE_WRITE,
            0,
        );
        if handle == INVALID_HANDLE_VALUE {
            Err(io::Error::last_os_error())
        } else {
            Ok(FromRawHandle::from_raw_handle(handle as RawHandle))
        }
    }
}

pub fn keep(path: &Path) -> io::Result<()> {
    unsafe {
        let path_w = to_utf16(path)?;
        if SetFileAttributesW(path_w.as_ptr(), FILE_ATTRIBUTE_NORMAL) == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

pub fn persist(old_path: &Path, new_path: &Path, overwrite: bool) -> io::Result<()> {
    unsafe {
        let old_path_w = to_utf16(old_path)?;
        let new_path_w = to_utf16(new_path)?;

        // Don't succeed if this fails. We don't want to claim to have successfully persisted a file
        // still marked as temporary because this file won't have the same consistency guarantees.
        if SetFileAttributesW(old_path_w.as_ptr(), FILE_ATTRIBUTE_NORMAL) == 0 {
            return Err(io::Error::last_os_error());
        }

        let mut flags = 0;

        if overwrite {
            flags |= MOVEFILE_REPLACE_EXISTING;
        }

        if MoveFileExW(old_path_w.as_ptr(), new_path_w.as_ptr(), flags) == 0 {
            let e = io::Error::last_os_error();
            // If this fails, the temporary file is now un-hidden and no longer marked temporary
            // (slightly less efficient) but it will still work.
            let _ = SetFileAttributesW(old_path_w.as_ptr(), FILE_ATTRIBUTE_TEMPORARY);
            Err(e)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod long_path_tests {
    use super::*;
    use std::{io::Write, iter};

    #[test]
    fn long_unicode_persistence_preserves_contents_and_non_overwrite_semantics() {
        let root = crate::tempdir().unwrap();
        let parent = root.path().join("a".repeat(120)).join("b".repeat(120));
        std::fs::create_dir_all(&parent).unwrap();
        let destination = parent.join("완료-🙂.tmp");
        let mut source = crate::NamedTempFile::new_in(&parent).unwrap();
        source.write_all(b"synthetic persisted bytes").unwrap();
        source.persist(&destination).unwrap();
        assert_eq!(
            std::fs::read(&destination).unwrap(),
            b"synthetic persisted bytes"
        );
        let source = crate::NamedTempFile::new_in(&parent).unwrap();
        let error = source.persist_noclobber(&destination).unwrap_err();
        assert!(error.file.path().exists());
        assert_eq!(
            std::fs::read(&destination).unwrap(),
            b"synthetic persisted bytes"
        );
    }

    #[test]
    fn short_paths_stay_unchanged_and_long_unc_paths_keep_their_share() {
        assert_eq!(
            to_utf16(Path::new("relative.tmp")).unwrap(),
            "relative.tmp"
                .encode_utf16()
                .chain(iter::once(0))
                .collect::<Vec<_>>()
        );
        let unc = format!("\\\\server\\share\\{}\\file.tmp", "segment\\".repeat(40));
        let value = to_utf16(Path::new(&unc)).unwrap();
        assert!(String::from_utf16(&value[..value.len() - 1])
            .unwrap()
            .starts_with("\\\\?\\UNC\\server\\share\\"));
    }
}

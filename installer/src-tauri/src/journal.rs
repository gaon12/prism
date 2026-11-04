//! Durable transaction phases and checked backup metadata. A journal is not permission to
//! write an arbitrary recorded path: the transaction loader applies destination boundaries.
use crate::safe_path;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

pub const STATE_FILE: &str = "state.json";
const MAX_STATE_BYTES: u64 = 16384;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    Preparing,
    Changing,
    Completed,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FileDigest {
    pub length: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Journal {
    pub schema_version: u32,
    pub install_root: PathBuf,
    pub phase: Phase,
    pub registration: Option<FileDigest>,
    pub files: Option<FileDigest>,
}

pub fn invalid() -> io::Error {
    io::Error::other("Installation recovery data is invalid or unsafe; backups were kept.")
}

pub fn workspace_name(name: &str) -> bool {
    let Some(suffix) = name.strip_prefix(".prism-setup-") else {
        return false;
    };
    let Some((pid, counter)) = suffix.split_once('-') else {
        return false;
    };
    !pid.is_empty()
        && !counter.is_empty()
        && pid.parse::<u32>().is_ok()
        && counter.parse::<u64>().is_ok()
        && pid.bytes().all(|b| b.is_ascii_digit())
        && counter.bytes().all(|b| b.is_ascii_digit())
}

pub fn read_bounded<T: DeserializeOwned>(path: &Path, limit: u64) -> io::Result<T> {
    safe_path::no_links(path)?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err(invalid());
    }
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(invalid());
    }
    serde_json::from_slice(&bytes).map_err(|_| invalid())
}

pub fn digest(path: &Path) -> io::Result<FileDigest> {
    safe_path::no_links(path)?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.len() > 2 * 1024 * 1024 * 1024 {
        return Err(invalid());
    }
    let mut file = fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    let mut length = 0u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        length = length.checked_add(read as u64).ok_or_else(invalid)?;
        if length > 2 * 1024 * 1024 * 1024 {
            return Err(invalid());
        }
        hash.update(&buffer[..read]);
    }
    Ok(FileDigest {
        length,
        sha256: format!("{:x}", hash.finalize()),
    })
}

impl Journal {
    pub fn new(root: &Path) -> Self {
        Self {
            schema_version: 1,
            install_root: root.into(),
            phase: Phase::Preparing,
            registration: None,
            files: None,
        }
    }
    pub fn open(root: &Path, directory: &Path) -> io::Result<Self> {
        let root = safe_path::root(root)?;
        let directory = checked_workspace(&root, directory)?;
        let journal: Self = read_bounded(&directory.join(STATE_FILE), MAX_STATE_BYTES)?;
        if journal.schema_version != 1 || journal.install_root != root {
            return Err(invalid());
        }
        Ok(journal)
    }
    pub fn save(&self, directory: &Path) -> io::Result<()> {
        let directory = checked_workspace(&self.install_root, directory)?;
        let pending = safe_path::file(&directory, "state.next")?;
        // A retry can replace only this protocol's exact incomplete state file.
        match fs::remove_file(&pending) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&pending)?;
        file.write_all(&serde_json::to_vec_pretty(self)?)?;
        file.sync_all()?;
        drop(file);
        let target = safe_path::file(&directory, STATE_FILE)?;
        atomic_replace(&pending, &target)?;
        Ok(())
    }
}

pub fn checked_workspace(root: &Path, directory: &Path) -> io::Result<PathBuf> {
    safe_path::no_links(directory)?;
    let root = safe_path::root(root)?;
    let directory = safe_path::root(directory)?;
    if directory.parent() != Some(root.as_path())
        || !directory
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(workspace_name)
    {
        return Err(invalid());
    }
    Ok(directory)
}

fn atomic_replace(from: &Path, to: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::{
            core::PCWSTR,
            Win32::Storage::FileSystem::{
                MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
            },
        };
        let source: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
        let target: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
        // SAFETY: both UTF-16 strings are terminated and live through the call.
        unsafe {
            MoveFileExW(
                PCWSTR(source.as_ptr()),
                PCWSTR(target.as_ptr()),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        }
        .map_err(|_| io::Error::last_os_error())?;
    }
    #[cfg(not(windows))]
    {
        fs::rename(from, to)?;
        fs::File::open(to.parent().ok_or_else(invalid)?)?.sync_all()?;
    }
    Ok(())
}

/// Keep state.json until all other recovery files are removed. A crash during cleanup
/// must not make a committed installation look like an interrupted replacement.
pub fn cleanup(root: &Path, directory: &Path) -> io::Result<()> {
    let directory = checked_workspace(root, directory)?;
    let mut stack = vec![directory.clone()];
    let mut count = 0usize;
    while let Some(folder) = stack.pop() {
        for entry in fs::read_dir(folder)? {
            let entry = entry?;
            let path = entry.path();
            safe_path::no_links(&path)?;
            count += 1;
            if count > 50000 {
                return Err(invalid());
            }
            if entry.file_type()?.is_dir() {
                stack.push(path);
            }
        }
    }
    for entry in fs::read_dir(&directory)? {
        let entry = entry?;
        if entry.file_name() == STATE_FILE {
            continue;
        }
        safe_path::no_links(&entry.path())?;
        if entry.file_type()?.is_dir() {
            fs::remove_dir_all(entry.path())?;
        } else {
            fs::remove_file(entry.path())?;
        }
    }
    match fs::remove_file(directory.join(STATE_FILE)) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    fs::remove_dir(directory)
}

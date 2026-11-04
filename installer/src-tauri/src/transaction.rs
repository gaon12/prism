//! Keep prior files until setup completes. Ordinary failures restore their exact bytes.
//! Durable phases let a later setup distinguish rollback from completed-install cleanup.

use crate::{
    journal::{self, FileDigest, Journal, Phase},
    payload, safe_path,
};
use std::{
    collections::HashSet,
    fs, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    root: PathBuf,
    name: String,
    backup: Option<PathBuf>,
    digest: Option<FileDigest>,
}
pub struct Files {
    root: PathBuf,
    pub directory: PathBuf,
    entries: Vec<Snapshot>,
    seen: HashSet<PathBuf>,
    retain: bool,
    journal: Journal,
}
impl Files {
    pub fn new(root: &Path) -> io::Result<Self> {
        let root = safe_path::root(root)?;
        for _ in 0..128 {
            let directory = root.join(format!(
                ".prism-setup-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&directory) {
                Ok(()) => {
                    let journal = Journal::new(&root);
                    journal.save(&directory)?;
                    fs::create_dir(directory.join("new"))?;
                    fs::create_dir(directory.join("backup"))?;
                    return Ok(Self {
                        root,
                        directory,
                        entries: vec![],
                        seen: HashSet::new(),
                        retain: false,
                        journal,
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::other(
            "Could not create an installation staging folder.",
        ))
    }
    pub fn stage(&self) -> PathBuf {
        self.directory.join("new")
    }
    /// Use the exact original bytes already captured for failure recovery, not
    /// another read of a live file that could have changed during preparation.
    pub fn original(&self, root: &Path, name: &str) -> io::Result<PathBuf> {
        let root = safe_path::root(root)?;
        let entry = self
            .entries
            .iter()
            .find(|e| e.root == root && e.name == name)
            .ok_or_else(journal::invalid)?;
        let backup = entry.backup.as_ref().ok_or_else(journal::invalid)?;
        if entry.digest.as_ref() != Some(&journal::digest(backup)?) {
            return Err(journal::invalid());
        }
        Ok(backup.clone())
    }
    pub fn snapshot(&mut self, root: &Path, name: &str) -> io::Result<()> {
        let root = safe_path::root(root)?;
        let target = safe_path::file(&root, name)?;
        // Windows aliases were rejected before this step. Normalize case for duplicate plans.
        #[cfg(windows)]
        let key = PathBuf::from(target.to_string_lossy().to_lowercase());
        #[cfg(not(windows))]
        let key = target.clone();
        if !self.seen.insert(key) {
            return Ok(());
        }
        let backup = match fs::symlink_metadata(&target) {
            Ok(_) => {
                let backup = self
                    .directory
                    .join("backup")
                    .join(self.entries.len().to_string());
                let mut output = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&backup)?;
                std::io::copy(&mut fs::File::open(&target)?, &mut output)?;
                output.sync_all()?;
                fs::set_permissions(&backup, fs::metadata(&target)?.permissions())?;
                Some(backup)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        let digest = backup.as_deref().map(journal::digest).transpose()?;
        self.entries.push(Snapshot {
            root,
            name: name.into(),
            backup,
            digest,
        });
        Ok(())
    }
    pub fn restore(&mut self) -> io::Result<()> {
        self.validate_backups()?;
        let mut failure = None;
        for entry in self.entries.iter().rev() {
            let result = (|| {
                let target = safe_path::file(&entry.root, &entry.name)?;
                if let Some(backup) = &entry.backup {
                    self.publish(backup, &entry.root, &entry.name)?;
                } else {
                    match fs::remove_file(&target) {
                        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
                        _ => {}
                    }
                    let mut parent = target.parent();
                    while let Some(folder) = parent.filter(|path| **path != entry.root) {
                        safe_path::no_links(folder)?;
                        if fs::remove_dir(folder).is_err() {
                            break;
                        }
                        parent = folder.parent();
                    }
                }
                Ok(())
            })();
            if let Err(error) = result {
                failure = Some(error);
            }
        }
        if let Some(error) = failure {
            self.retain = true;
            Err(error)
        } else {
            Ok(())
        }
    }
    /// Publish complete bytes on the destination's own volume. Never truncate a live
    /// shortcut or follow an existing hard link while saving COM output.
    pub fn publish(&self, source: &Path, root: &Path, name: &str) -> io::Result<()> {
        safe_path::no_links(source)?;
        let target = safe_path::create_parent(root, name)?;
        let token = self
            .directory
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(journal::invalid)?;
        let temporary_name = format!("{name}.prism-recover-{token}");
        let temporary = safe_path::file(root, &temporary_name)?;
        match fs::remove_file(&temporary) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        let copied = (|| {
            std::io::copy(&mut fs::File::open(source)?, &mut output)?;
            output.sync_all()?;
            fs::set_permissions(&temporary, fs::metadata(source)?.permissions())?;
            Ok::<(), io::Error>(())
        })();
        drop(output);
        if let Err(error) = copied {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        safe_path::file(root, name)?;
        payload::replace_file(&temporary, &target)
    }

    pub fn check_shortcut_ownership(&self, system: &dyn crate::install::System) -> io::Result<()> {
        for entry in &self.entries {
            if safe_path::root(&entry.root)? == self.root {
                continue;
            }
            let link = safe_path::file(&entry.root, &entry.name)?;
            if link.exists()
                && !system
                    .shortcut_points_to(&link, &self.root.join(crate::install::APP_EXE))
                    .unwrap_or(false)
            {
                return Err(io::Error::other(
                    "A shortcut changed after setup was interrupted.",
                ));
            }
        }
        Ok(())
    }

    pub fn retain(&mut self) {
        self.retain = true;
    }
    /// Persist the backup map before the first installed file is changed. If an
    /// unexpected unwind occurs, Drop keeps these files instead of deleting recovery data.
    pub fn begin_changes(&mut self) -> io::Result<()> {
        use std::io::Write;
        let path = self.directory.join("files.json");
        safe_path::no_links(&path)?;
        let mut journal = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        journal.write_all(&serde_json::to_vec_pretty(&self.entries)?)?;
        journal.sync_all()?;
        drop(journal);
        self.journal.files = Some(journal::digest(&self.directory.join("files.json"))?);
        let registration = self.directory.join("registration.json");
        if registration.exists() {
            fs::OpenOptions::new()
                .write(true)
                .open(&registration)?
                .sync_all()?;
            self.journal.registration = Some(journal::digest(&registration)?);
        }
        self.retain = true;
        self.journal.phase = Phase::Changing;
        self.journal.save(&self.directory)?;
        Ok(())
    }
    pub fn finished(&mut self) -> io::Result<()> {
        let mut completed = self.journal.clone();
        completed.phase = Phase::Completed;
        completed.save(&self.directory)?;
        self.journal = completed;
        self.retain = false;
        Ok(())
    }
    pub fn needs_restore(&self) -> bool {
        self.journal.phase == Phase::Changing
    }

    /// Estimate from the checked recovery map without reading large backup bodies on the UI thread.
    pub fn planned_backup_bytes(root: &Path, directory: &Path) -> io::Result<Option<u64>> {
        let directory = journal::checked_workspace(root, directory)?;
        let state = Journal::open(root, &directory)?;
        if state.phase != Phase::Changing {
            return Ok(None);
        }
        let map = directory.join("files.json");
        if state.files.as_ref() != Some(&journal::digest(&map)?) {
            return Err(journal::invalid());
        }
        let entries: Vec<Snapshot> = journal::read_bounded(&map, 32 * 1024 * 1024)?;
        if entries.len() > 20010 {
            return Err(journal::invalid());
        }
        let mut total = 0u64;
        for (index, entry) in entries.iter().enumerate() {
            match (&entry.backup, &entry.digest) {
                (None, None) => {}
                (Some(path), Some(digest))
                    if *path == directory.join("backup").join(index.to_string())
                        && digest.length <= 2 * 1024 * 1024 * 1024 =>
                {
                    total = total
                        .checked_add(digest.length)
                        .ok_or_else(journal::invalid)?;
                }
                _ => return Err(journal::invalid()),
            }
        }
        Ok(Some(total))
    }

    fn validate_backups(&self) -> io::Result<()> {
        for (index, entry) in self.entries.iter().enumerate() {
            match (&entry.backup, &entry.digest) {
                (Some(path), Some(expected)) => {
                    if *path != self.directory.join("backup").join(index.to_string())
                        || journal::digest(path)? != *expected
                    {
                        return Err(journal::invalid());
                    }
                }
                (None, None) => {}
                _ => return Err(journal::invalid()),
            }
        }
        Ok(())
    }

    /// Load every destination and backup before the caller performs a single recovery write.
    pub fn load(root: &Path, directory: &Path, shortcut_roots: &[PathBuf]) -> io::Result<Self> {
        let root = safe_path::root(root)?;
        let directory = journal::checked_workspace(&root, directory)?;
        let state = Journal::open(&root, &directory)?;
        let mut files = Self {
            root: root.clone(),
            directory,
            entries: vec![],
            seen: HashSet::new(),
            retain: true,
            journal: state,
        };
        if files.needs_restore() {
            if files.journal.files.as_ref()
                != Some(&journal::digest(&files.directory.join("files.json"))?)
            {
                return Err(journal::invalid());
            }
            files.entries =
                journal::read_bounded(&files.directory.join("files.json"), 32 * 1024 * 1024)?;
            if files.entries.len() > 20010 {
                return Err(journal::invalid());
            }
            for entry in &files.entries {
                let destination_root = safe_path::root(&entry.root)?;
                if destination_root != root
                    && (entry.name != "Prism.lnk"
                        || !shortcut_roots.iter().any(|allowed| {
                            safe_path::root(allowed)
                                .is_ok_and(|allowed| allowed == destination_root)
                        }))
                {
                    return Err(journal::invalid());
                }
                if entry.name.to_ascii_lowercase().starts_with(".prism-") {
                    return Err(journal::invalid());
                }
                let target = safe_path::file(&destination_root, &entry.name)?;
                #[cfg(windows)]
                let target = PathBuf::from(target.to_string_lossy().to_lowercase());
                if !files.seen.insert(target) {
                    return Err(journal::invalid());
                }
            }
            files.validate_backups()?;
            files.registration_snapshot()?;
        }
        Ok(files)
    }
    pub fn registration_snapshot(&self) -> io::Result<Option<Vec<u8>>> {
        use std::io::Read;
        let Some(expected) = &self.journal.registration else {
            return Ok(None);
        };
        let path = self.directory.join("registration.json");
        if expected.length > 16 * 1024 * 1024 || journal::digest(&path)? != *expected {
            return Err(journal::invalid());
        }
        let mut bytes = Vec::new();
        fs::File::open(path)?
            .take(16 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 != expected.length {
            return Err(journal::invalid());
        }
        Ok(Some(bytes))
    }
    pub fn cleanup(&mut self) -> io::Result<()> {
        journal::cleanup(&self.root, &self.directory)?;
        self.retain = true; // Directory is gone; Drop has nothing left to clean.
        Ok(())
    }
}

impl Drop for Files {
    fn drop(&mut self) {
        if self.retain {
            return;
        }
        let _ = journal::cleanup(&self.root, &self.directory);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn an_unexpected_unwind_keeps_the_backup_and_recovery_map() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("prism.exe"), b"old").unwrap();
        let directory = std::cell::RefCell::new(PathBuf::new());
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut files = Files::new(temp.path()).unwrap();
            files.snapshot(temp.path(), "prism.exe").unwrap();
            files.begin_changes().unwrap();
            *directory.borrow_mut() = files.directory.clone();
            fs::write(temp.path().join("prism.exe"), b"changed").unwrap();
            panic!("synthetic setup unwind");
        }));
        assert!(result.is_err());
        let directory = directory.into_inner();
        assert!(directory.join("files.json").exists());
        assert_eq!(fs::read(directory.join("backup/0")).unwrap(), b"old");
    }
}

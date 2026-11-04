//! Keep prior files until setup completes. Ordinary failures restore their exact bytes.
//! An interrupted process may leave recovery files; crash recovery is a separate protocol.

use crate::{payload, safe_path};
use std::{
    collections::HashSet,
    fs, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
#[derive(serde::Serialize)]
struct Snapshot {
    root: PathBuf,
    name: String,
    backup: Option<PathBuf>,
}
pub struct Files {
    root: PathBuf,
    pub directory: PathBuf,
    entries: Vec<Snapshot>,
    seen: HashSet<PathBuf>,
    retain: bool,
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
                    fs::create_dir(directory.join("new"))?;
                    fs::create_dir(directory.join("backup"))?;
                    return Ok(Self {
                        root,
                        directory,
                        entries: vec![],
                        seen: HashSet::new(),
                        retain: false,
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
                fs::copy(&target, &backup)?;
                Some(backup)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        self.entries.push(Snapshot {
            root,
            name: name.into(),
            backup,
        });
        Ok(())
    }
    pub fn restore(&mut self) -> io::Result<()> {
        let mut failure = None;
        for entry in self.entries.iter().rev() {
            let result = (|| {
                let target = safe_path::file(&entry.root, &entry.name)?;
                if let Some(backup) = &entry.backup {
                    safe_path::no_links(backup)?;
                    safe_path::create_parent(&entry.root, &entry.name)?;
                    // Shortcuts may be on a different drive from the install folder.
                    // Copy into a new file beside the destination, then rename locally.
                    // Keep the original backup for incomplete recovery or another attempt.
                    let temporary_name = format!(
                        "{}.prism-recover-{}-{}",
                        entry.name,
                        std::process::id(),
                        NEXT.fetch_add(1, Ordering::Relaxed)
                    );
                    let temporary = safe_path::file(&entry.root, &temporary_name)?;
                    let mut output = fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&temporary)?;
                    let copied = (|| {
                        std::io::copy(&mut fs::File::open(backup)?, &mut output)?;
                        output.sync_all()?;
                        fs::set_permissions(&temporary, fs::metadata(backup)?.permissions())?;
                        Ok::<(), io::Error>(())
                    })();
                    drop(output);
                    if let Err(error) = copied {
                        let _ = fs::remove_file(&temporary);
                        return Err(error);
                    }
                    safe_path::file(&entry.root, &entry.name)?;
                    payload::replace_file(&temporary, &target)?;
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
        self.retain = true;
        Ok(())
    }
    pub fn finished(&mut self) {
        self.retain = false;
    }
}

impl Drop for Files {
    fn drop(&mut self) {
        if self.retain {
            return;
        }
        // Recursive cleanup is limited to the exact private workspace created above.
        if safe_path::no_links(&self.directory).is_ok()
            && fs::canonicalize(&self.directory)
                .ok()
                .is_some_and(|dir| dir.parent() == Some(self.root.as_path()))
        {
            let _ = fs::remove_dir_all(&self.directory);
        }
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

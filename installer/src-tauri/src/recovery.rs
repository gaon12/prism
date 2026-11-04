//! Resume one interrupted installation under the same folder lock used for new work.
//! Never infer a cleanup/restore plan from a directory name alone.
use crate::{install::System, journal, safe_path, transaction::Files};
use std::{fs, path::Path};

pub fn has_workspaces(root: &Path) -> bool {
    fs::read_dir(root).is_ok_and(|entries| {
        entries.filter_map(Result::ok).any(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(journal::workspace_name)
        })
    })
}

/// A space check remains read-only even when the current install record is damaged.
/// Original sizes come from the checksummed map, not partially replaced live files.
pub fn pending_backup_bytes(root: &Path) -> Result<Option<u64>, String> {
    let mut total = None;
    for entry in
        fs::read_dir(root).map_err(|_| "Could not inspect recovery metadata.".to_string())?
    {
        let entry = entry.map_err(|_| "Could not inspect recovery metadata.".to_string())?;
        if !entry
            .file_name()
            .to_str()
            .is_some_and(journal::workspace_name)
        {
            continue;
        }
        if !entry.path().join(journal::STATE_FILE).exists()
            && !entry.path().join("files.json").exists()
        {
            continue;
        }
        if let Some(bytes) = Files::planned_backup_bytes(root, &entry.path())
            .map_err(|_| "Recovery metadata could not be verified.".to_string())?
        {
            if total.is_some() {
                return Err("Multiple interrupted installations need review.".into());
            }
            total = Some(bytes);
        }
    }
    Ok(total)
}

pub fn recover(system: &dyn System, root: &Path) -> Result<usize, String> {
    let root = safe_path::root(root).map_err(|_| "The recovery folder is unsafe.".to_string())?;
    let folders = system
        .folders()
        .map_err(|_| "Recovery could not locate application folders.".to_string())?;
    let shortcut_roots = [folders.desktop, folders.start_menu];
    let mut candidates = Vec::new();
    for entry in fs::read_dir(&root)
        .map_err(|_| "Could not inspect installation recovery files.".to_string())?
    {
        let entry =
            entry.map_err(|_| "Could not inspect installation recovery files.".to_string())?;
        if !entry
            .file_name()
            .to_str()
            .is_some_and(journal::workspace_name)
        {
            continue;
        }
        let directory = journal::checked_workspace(&root, &entry.path()).map_err(|_| {
            "An installation recovery folder is unsafe; it was preserved.".to_string()
        })?;
        // A process can stop before publishing its first state. There were no authorized
        // installed-file mutations yet. Preserve this staging directory, rather than delete
        // unknown files. Old journals without a state are ambiguous and fail closed.
        if !directory.join(journal::STATE_FILE).exists() && !directory.join("files.json").exists() {
            continue;
        }
        if candidates.len() == 16 {
            return Err("Too many recovery folders; all were preserved.".into());
        }
        candidates.push(Files::load(&root, &directory, &shortcut_roots).map_err(|_| "Installation recovery data is damaged or unsafe; no recovery writes were started.".to_string())?);
    }
    let interrupted = candidates
        .iter()
        .filter(|files| files.needs_restore())
        .count();
    if interrupted > 1 {
        return Err(
            "Multiple interrupted installations need review; all backups were preserved.".into(),
        );
    }
    for files in candidates.iter().filter(|files| files.needs_restore()) {
        files.check_shortcut_ownership(system).map_err(|_| {
            "A shortcut changed after setup was interrupted. It and the backups were preserved."
                .to_string()
        })?;
    }
    // Validate the registration snapshot before any restoration. Its OS adapter writes
    // only the fixed per-user Prism registration (or the isolated sandbox entry).
    let registration = candidates
        .iter()
        .find(|files| files.needs_restore())
        .map(|files| {
            files
                .registration_snapshot()
                .map_err(|_| "The registration backup is damaged.".to_string())?
                .ok_or_else(|| {
                    "The interrupted installation has no registration backup.".to_string()
                })
        })
        .transpose()?;
    for mut files in candidates {
        if files.needs_restore() {
            files.restore().map_err(|_| "Recovery could not restore application files. Close Prism and retry; backups were kept.".to_string())?;
            system
                .restore_registration(
                    registration
                        .as_deref()
                        .ok_or("Registration recovery is unavailable.")?,
                )
                .map_err(|_| {
                    "Recovery could not restore Windows registration; backups were kept."
                        .to_string()
                })?;
        }
        files
            .finished()
            .map_err(|_| "Could not finalize recovery; backups were kept.".to_string())?;
        files.cleanup().map_err(|_| {
            "Recovery finished, but its backup folder could not be cleaned up. Retry setup."
                .to_string()
        })?;
    }
    Ok(interrupted)
}

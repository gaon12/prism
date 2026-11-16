//! Manual backup and restore of the person's own Prism data.
//!
//! What a backup holds (all plain files, so it can be copied anywhere):
//! - `settings.json` and `session.json` (search folders, preferences, recent searches)
//! - `conversations.sqlite3` (AI chats) and `documents.sqlite3` (documents and revisions)
//! - `manifest.json`, written last: a folder without it is an unfinished backup.
//!
//! What it deliberately leaves out:
//! - API keys. They stay in the system keychain, tied to this computer account.
//! - Search indexes and model files. They are rebuilt from the original files.
//! - Clipboard history. It can hold passwords and is meant to stay on this computer.
//!
//! Restoring never replaces open databases. It checks the backup, copies it into
//! `<data>/restore-pending`, and the next start moves the files into place before any
//! database is opened. The data that was replaced is kept once in `<data>/restore-previous`.
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const FORMAT: &str = "prism-backup";
const VERSION: u32 = 1;
const MANIFEST: &str = "manifest.json";
const PENDING: &str = "restore-pending";
const PREVIOUS: &str = "restore-previous";
/// A settings or session file larger than this is not ours.
const MAX_JSON_BYTES: u64 = 16 << 20;

/// Where each backed-up file lives in a running profile.
#[derive(Clone, Copy)]
enum Place {
    /// The settings/profile folder.
    Data,
    /// The local-data folder that holds caches and databases.
    Assets(&'static str),
}
/// The only files a backup may contain, and where they go back to.
const FILES: [(&str, Place, bool); 4] = [
    ("settings.json", Place::Data, false),
    ("session.json", Place::Data, false),
    ("conversations.sqlite3", Place::Assets("rag"), true),
    ("documents.sqlite3", Place::Assets("documents"), true),
];

/// The two folders a profile uses. They are the same folder for portable copies.
pub struct Profile<'a> {
    pub data_dir: &'a Path,
    pub asset_dir: &'a Path,
}
impl Profile<'_> {
    fn path(&self, name: &str, place: Place) -> PathBuf {
        match place {
            Place::Data => self.data_dir.join(name),
            Place::Assets(folder) => self.asset_dir.join(folder).join(name),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    format: String,
    version: u32,
    created_ms: u64,
    files: Vec<String>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis() as u64)
        .unwrap_or_default()
}
fn error(message: impl Into<String>) -> String {
    message.into()
}
fn io_error(context: &str) -> impl Fn(io::Error) -> String + '_ {
    move |cause| format!("{context}: {cause}")
}

/// Writes a new backup folder inside `parent` and returns its path.
///
/// Databases are copied with SQLite's `VACUUM INTO`, which reads one consistent snapshot
/// even while Prism is writing, so the copy is never half old and half new.
pub fn create(profile: &Profile, parent: &Path) -> Result<PathBuf, String> {
    if !parent.is_dir() {
        return Err(error("Choose an existing folder for the backup."));
    }
    let stamp = chrono_like_stamp(now_ms());
    let mut target = parent.join(format!("Prism backup {stamp}"));
    let mut suffix = 2;
    while target.exists() {
        target = parent.join(format!("Prism backup {stamp} ({suffix})"));
        suffix += 1;
    }
    fs::create_dir(&target).map_err(io_error("Could not create the backup folder"))?;
    let result = (|| {
        let mut files = Vec::new();
        for (name, place, database) in FILES {
            let source = profile.path(name, place);
            if !source.is_file() {
                continue;
            }
            let destination = target.join(name);
            if database {
                let db = Connection::open_with_flags(&source, OpenFlags::SQLITE_OPEN_READ_ONLY)
                    .map_err(|cause| format!("Could not read {name}: {cause}"))?;
                db.execute("VACUUM INTO ?1", [destination.to_string_lossy()])
                    .map_err(|cause| format!("Could not copy {name}: {cause}"))?;
            } else {
                fs::copy(&source, &destination).map_err(io_error("Could not copy settings"))?;
            }
            files.push(name.to_string());
        }
        let manifest = Manifest {
            format: FORMAT.into(),
            version: VERSION,
            created_ms: now_ms(),
            files,
        };
        let json = serde_json::to_vec_pretty(&manifest).map_err(|cause| cause.to_string())?;
        fs::write(target.join(MANIFEST), json).map_err(io_error("Could not finish the backup"))
    })();
    if let Err(message) = result {
        // Leave nothing that looks like a backup behind.
        let _ = fs::remove_dir_all(&target);
        return Err(message);
    }
    Ok(target)
}

/// Checks a backup folder and stages it to replace this profile's data at the next start.
pub fn stage_restore(profile: &Profile, backup: &Path) -> Result<(), String> {
    let manifest = read_manifest(backup)?;
    let pending = profile.data_dir.join(PENDING);
    let staging = profile.data_dir.join(format!("{PENDING}.tmp"));
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging).map_err(io_error("Could not prepare the restore"))?;
    let result = (|| {
        for name in &manifest.files {
            let (_, _, database) = known(name)?;
            let source = backup.join(name);
            let meta = fs::symlink_metadata(&source)
                .map_err(|_| format!("The backup is missing {name}."))?;
            if !meta.is_file() {
                return Err(format!("The backup is missing {name}."));
            }
            if database {
                check_database(&source, name)?;
            } else if meta.len() > MAX_JSON_BYTES
                || serde_json::from_slice::<serde_json::Value>(
                    &fs::read(&source).map_err(io_error("Could not read the backup"))?,
                )
                .is_err()
            {
                return Err(format!("{name} in the backup is damaged."));
            }
            fs::copy(&source, staging.join(name)).map_err(io_error("Could not copy the backup"))?;
        }
        let json = serde_json::to_vec_pretty(&manifest).map_err(|cause| cause.to_string())?;
        fs::write(staging.join(MANIFEST), json)
            .map_err(io_error("Could not prepare the restore"))?;
        // Swap in the finished staging folder; a crash before this leaves no pending restore.
        let _ = fs::remove_dir_all(&pending);
        fs::rename(&staging, &pending).map_err(io_error("Could not prepare the restore"))
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

/// What happened to a staged restore when this run started (shown once in Settings).
static STARTUP: std::sync::OnceLock<Result<bool, String>> = std::sync::OnceLock::new();

/// Applies a staged restore at startup and remembers the outcome for `startup_outcome`.
/// A failure is reported, never fatal: Prism then starts with the data it had.
pub fn apply_at_startup(profile: &Profile) {
    let outcome = apply_pending(profile);
    if let Err(message) = &outcome {
        eprintln!("Prism could not restore the backup: {message}");
    }
    let _ = STARTUP.set(outcome);
}
/// `Ok(true)` when this run started from a restored backup, `Err` when that failed.
pub fn startup_outcome() -> Result<bool, String> {
    STARTUP.get().cloned().unwrap_or(Ok(false))
}

/// True when a restore is waiting for the next start.
pub fn pending(profile: &Profile) -> bool {
    profile.data_dir.join(PENDING).join(MANIFEST).is_file()
}

/// Cancels a restore that has not been applied yet.
pub fn cancel_pending(profile: &Profile) -> Result<(), String> {
    match fs::remove_dir_all(profile.data_dir.join(PENDING)) {
        Err(cause) if cause.kind() != io::ErrorKind::NotFound => {
            Err(format!("Could not cancel the restore: {cause}"))
        }
        _ => Ok(()),
    }
}

/// At startup, before settings are read or any database is opened: moves a staged
/// restore into place. Returns true when one was applied.
///
/// Each replaced file is first moved to `restore-previous` (only the latest one is kept),
/// so a restore can be undone by hand. SQLite's side files (`-wal`, `-shm`) belong to the
/// old database and are moved with it.
pub fn apply_pending(profile: &Profile) -> Result<bool, String> {
    let pending = profile.data_dir.join(PENDING);
    let manifest = match read_manifest(&pending) {
        Ok(manifest) => manifest,
        Err(_) if !pending.exists() => return Ok(false),
        Err(message) => {
            let _ = fs::remove_dir_all(&pending);
            return Err(message);
        }
    };
    let previous = profile.data_dir.join(PREVIOUS);
    let _ = fs::remove_dir_all(&previous);
    fs::create_dir_all(&previous).map_err(io_error("Could not keep the replaced data"))?;
    for name in &manifest.files {
        let (_, place, database) = known(name)?;
        let destination = profile.path(name, place);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(io_error("Could not restore"))?;
        }
        let sides: &[&str] = if database {
            &["", "-wal", "-shm"]
        } else {
            &[""]
        };
        for side in sides {
            let current = PathBuf::from(format!("{}{side}", destination.display()));
            if current.exists() {
                fs::rename(&current, previous.join(format!("{name}{side}")))
                    .map_err(io_error("Could not keep the replaced data"))?;
            }
        }
        fs::rename(pending.join(name), &destination).map_err(io_error("Could not restore"))?;
    }
    let _ = fs::remove_dir_all(&pending);
    Ok(true)
}

fn known(name: &str) -> Result<(&'static str, Place, bool), String> {
    FILES
        .iter()
        .copied()
        .find(|(file, _, _)| *file == name)
        .ok_or_else(|| format!("The backup contains an unknown file: {name}"))
}

fn read_manifest(folder: &Path) -> Result<Manifest, String> {
    let bytes = fs::read(folder.join(MANIFEST))
        .map_err(|_| error("This folder is not a finished Prism backup."))?;
    let manifest: Manifest = serde_json::from_slice(&bytes)
        .map_err(|_| error("This folder is not a finished Prism backup."))?;
    if manifest.format != FORMAT {
        return Err(error("This folder is not a finished Prism backup."));
    }
    if manifest.version > VERSION {
        return Err(error(
            "This backup was made by a newer Prism. Update Prism first.",
        ));
    }
    let mut seen = std::collections::HashSet::new();
    for name in &manifest.files {
        known(name)?;
        if !seen.insert(name) {
            return Err(error("This folder is not a finished Prism backup."));
        }
    }
    Ok(manifest)
}

fn check_database(path: &Path, name: &str) -> Result<(), String> {
    let damaged = || format!("{name} in the backup is damaged.");
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| damaged())?;
    let result: String = db
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .map_err(|_| damaged())?;
    if result == "ok" {
        Ok(())
    } else {
        Err(damaged())
    }
}

/// `YYYY-MM-DD HHMMSS` in UTC, without pulling in a date library for one folder name.
fn chrono_like_stamp(ms: u64) -> String {
    let seconds = ms / 1000;
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    // Civil-from-days (Howard Hinnant's algorithm), valid for all dates after 1970.
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}{:02}{:02}",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn database(path: &Path, value: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let db = Connection::open(path).unwrap();
        db.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE IF NOT EXISTS t(v TEXT);")
            .unwrap();
        db.execute("DELETE FROM t", []).unwrap();
        db.execute("INSERT INTO t(v) VALUES(?1)", [value]).unwrap();
    }
    fn value(path: &Path) -> String {
        Connection::open(path)
            .unwrap()
            .query_row("SELECT v FROM t", [], |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn stamp_is_a_readable_utc_date() {
        // 2026-11-16 15:30:12 UTC
        assert_eq!(chrono_like_stamp(1_794_843_012_000), "2026-11-16 153012");
        assert_eq!(chrono_like_stamp(0), "1970-01-01 000000");
    }

    #[test]
    fn backup_then_restore_brings_back_settings_chats_and_documents() {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        let assets = root.path().join("assets");
        fs::create_dir_all(&data).unwrap();
        let profile = Profile {
            data_dir: &data,
            asset_dir: &assets,
        };
        fs::write(data.join("settings.json"), r#"{"language":"ko"}"#).unwrap();
        // session.json is absent: a backup holds only what exists.
        database(&assets.join("rag/conversations.sqlite3"), "chat before");
        database(&assets.join("documents/documents.sqlite3"), "doc before");
        // Keep a connection open, as the running app does, while backing up.
        let open = Connection::open(assets.join("rag/conversations.sqlite3")).unwrap();
        let backups = root.path().join("backups");
        fs::create_dir(&backups).unwrap();
        let backup = create(&profile, &backups).unwrap();
        drop(open);
        let manifest = read_manifest(&backup).unwrap();
        assert_eq!(
            manifest.files,
            [
                "settings.json",
                "conversations.sqlite3",
                "documents.sqlite3"
            ]
        );
        // A second backup in the same second gets its own folder.
        assert_ne!(create(&profile, &backups).unwrap(), backup);

        // Change everything, then restore.
        fs::write(data.join("settings.json"), r#"{"language":"en"}"#).unwrap();
        database(&assets.join("rag/conversations.sqlite3"), "chat after");
        database(&assets.join("documents/documents.sqlite3"), "doc after");
        stage_restore(&profile, &backup).unwrap();
        assert!(pending(&profile));
        // Nothing changes until the next start.
        assert_eq!(
            value(&assets.join("rag/conversations.sqlite3")),
            "chat after"
        );
        assert!(apply_pending(&profile).unwrap());
        assert!(!pending(&profile));
        assert_eq!(
            fs::read_to_string(data.join("settings.json")).unwrap(),
            r#"{"language":"ko"}"#
        );
        assert_eq!(
            value(&assets.join("rag/conversations.sqlite3")),
            "chat before"
        );
        assert_eq!(
            value(&assets.join("documents/documents.sqlite3")),
            "doc before"
        );
        // The replaced data is kept once, with its side files.
        assert_eq!(
            value(&data.join(PREVIOUS).join("documents.sqlite3")),
            "doc after"
        );
        // A second start has nothing to apply.
        assert!(!apply_pending(&profile).unwrap());
    }

    #[test]
    fn unfinished_damaged_or_foreign_backups_are_refused_without_changes() {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        fs::create_dir_all(&data).unwrap();
        let profile = Profile {
            data_dir: &data,
            asset_dir: &data,
        };
        let backup = root.path().join("backup");
        fs::create_dir(&backup).unwrap();
        // No manifest: an unfinished copy.
        assert!(stage_restore(&profile, &backup).is_err());
        let manifest = |files: &[&str], version: u32| {
            fs::write(
                backup.join(MANIFEST),
                serde_json::to_vec(&Manifest {
                    format: FORMAT.into(),
                    version,
                    created_ms: 0,
                    files: files.iter().map(|name| name.to_string()).collect(),
                })
                .unwrap(),
            )
            .unwrap();
        };
        // A file name outside the list (e.g. a path) is never copied anywhere.
        manifest(&["../settings.json"], 1);
        assert!(stage_restore(&profile, &backup).is_err());
        // A listed file that is missing.
        manifest(&["settings.json"], 1);
        assert!(stage_restore(&profile, &backup).is_err());
        // Damaged JSON and a database that is not SQLite.
        fs::write(backup.join("settings.json"), "{broken").unwrap();
        assert!(stage_restore(&profile, &backup).is_err());
        fs::write(backup.join("settings.json"), "{}").unwrap();
        fs::write(backup.join("documents.sqlite3"), "not a database").unwrap();
        manifest(&["settings.json", "documents.sqlite3"], 1);
        assert!(stage_restore(&profile, &backup).is_err());
        // A newer format.
        manifest(&["settings.json"], VERSION + 1);
        assert!(stage_restore(&profile, &backup).is_err());
        assert!(!pending(&profile));
        assert!(!data.join(format!("{PENDING}.tmp")).exists());
        // A valid one stages, and can be cancelled before it is applied.
        manifest(&["settings.json"], 1);
        stage_restore(&profile, &backup).unwrap();
        cancel_pending(&profile).unwrap();
        assert!(!apply_pending(&profile).unwrap());
    }
}

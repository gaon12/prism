//! A plain-text log of what setup did, so a failure can be explained afterwards.
//!
//! Every setup run (window, `/P`, `/S`, uninstall, repair) writes one file:
//! `%LOCALAPPDATA%\Prism Setup\logs\setup-YYYYMMDD-HHMMSS-<pid>.log`
//! (inside the sandbox folder in sandbox mode). Only the 10 newest log files are kept.
//!
//! What is written: start-up mode, folders, progress stages, errors and the result.
//! What is never written: Prism's own `/ARGS` values or any file contents.
//!
//! Logging must never break installing: every logging error is ignored, and without a
//! log folder setup simply runs without a log.

use crate::{install::System, safe_path};
use std::{
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

/// Folder under `%LOCALAPPDATA%`. Not the app's own data folder, so removing Prism's data
/// on uninstall keeps the log that explains the removal.
const LOG_FOLDER: [&str; 2] = ["Prism Setup", "logs"];
/// How many log files are kept; older ones are deleted when a new run starts.
const KEEP: usize = 10;
/// The window shows at most this much of the end of the log.
pub const TAIL_BYTES: u64 = 64 * 1024;

struct Log {
    path: PathBuf,
    file: Mutex<fs::File>,
}
static LOG: OnceLock<Option<Log>> = OnceLock::new();

/// Opens this run's log file. Call once at start-up; later calls do nothing.
pub fn start(system: &dyn System) {
    LOG.get_or_init(|| open(system).ok());
}

fn open(system: &dyn System) -> std::io::Result<Log> {
    let folders = system.folders()?;
    // Create the folders one level at a time and refuse links (junctions) at every level
    // *before* writing into it, so a planted link cannot make setup create files elsewhere.
    let mut dir = folders.local_app_data.clone();
    safe_path::no_links(&dir)?;
    for part in LOG_FOLDER {
        dir.push(part);
        match fs::create_dir(&dir) {
            Err(error) if error.kind() != std::io::ErrorKind::AlreadyExists => return Err(error),
            _ => {}
        }
        safe_path::no_links(&dir)?;
    }
    remove_old_logs(&dir, KEEP - 1);
    let name = format!(
        "setup-{}-{}.log",
        timestamp(now_secs())
            .replace(['-', ':'], "")
            .replace('T', "-"),
        std::process::id()
    );
    let path = safe_path::file(&dir, &name)?;
    let mut options = fs::OpenOptions::new();
    options.create_new(true).append(true).read(true);
    // Windows: let others read and append but not delete, so another setup's cleanup of old
    // logs cannot remove the log of a setup that is still running.
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 0x1;
        const FILE_SHARE_WRITE: u32 = 0x2;
        options.share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE);
    }
    let file = options.open(&path)?;
    Ok(Log {
        path,
        file: Mutex::new(file),
    })
}

/// Deletes the oldest `setup-*.log` files so that at most `keep` remain. Other files in the
/// folder are never touched.
fn remove_old_logs(dir: &Path, keep: usize) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut logs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("setup-") && name.ends_with(".log"))
        })
        .collect();
    // Names start with the time, so name order is age order.
    logs.sort();
    let extra = logs.len().saturating_sub(keep);
    for old in &logs[..extra] {
        let _ = fs::remove_file(old);
    }
}

/// Writes one line, prefixed with the UTC time. Line breaks inside `message` are flattened
/// so one event is always one line.
pub fn line(message: impl AsRef<str>) {
    let Some(Some(log)) = LOG.get() else {
        return;
    };
    let text = message.as_ref().replace(['\r', '\n'], " ");
    if let Ok(mut file) = log.file.lock() {
        let _ = writeln!(file, "{}  {}", timestamp(now_secs()), text);
        let _ = file.flush();
    }
}

/// Path of this run's log file, if one could be created.
pub fn path() -> Option<PathBuf> {
    LOG.get()?.as_ref().map(|log| log.path.clone())
}

/// The end of a log for the window.
#[derive(Debug, PartialEq, Eq)]
pub struct Tail {
    pub text: String,
    /// True when older lines (or the start of one long line) were left out.
    pub truncated: bool,
}

/// The last `TAIL_BYTES` of this run's log, starting at a whole line when possible.
/// Reads through the same file handle as the writer, under its lock, so a line being written
/// is never cut in half and the file cannot be swapped by path. Errors when there is no log
/// or it cannot be read.
pub fn tail() -> std::io::Result<Tail> {
    let log = LOG
        .get()
        .and_then(Option::as_ref)
        .ok_or_else(|| std::io::Error::other("no setup log"))?;
    let mut file = log
        .file
        .lock()
        .map_err(|_| std::io::Error::other("setup log unavailable"))?;
    read_tail_from(&mut file, TAIL_BYTES)
}

#[cfg(test)]
fn read_tail(path: &Path, max: u64) -> std::io::Result<String> {
    read_tail_from(&mut fs::File::open(path)?, max).map(|tail| tail.text)
}

/// Appends always go to the end of the file, so moving the read position here is harmless.
fn read_tail_from(file: &mut fs::File, max: u64) -> std::io::Result<Tail> {
    let length = file.metadata()?.len();
    let start = length.saturating_sub(max);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(max).read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    // When cut in the middle, drop the partial first line, unless the whole tail is one very
    // long line: then show its end rather than nothing.
    let text = match text.split_once('\n') {
        Some((_, rest)) if start > 0 && !rest.is_empty() => rest.to_owned(),
        _ => text,
    };
    Ok(Tail {
        text,
        truncated: start > 0,
    })
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|time| time.as_secs())
        .unwrap_or(0)
}

/// `YYYY-MM-DDTHH:MM:SSZ` for seconds since 1970 (UTC), without a date library.
fn timestamp(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rest = secs % 86_400;
    // Days since 1970-01-01 to a calendar date (Howard Hinnant's civil_from_days).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_are_utc_calendar_times() {
        assert_eq!(timestamp(0), "1970-01-01T00:00:00Z");
        // 2000-02-29 (leap day) 12:34:56.
        assert_eq!(timestamp(951_827_696), "2000-02-29T12:34:56Z");
        assert_eq!(timestamp(1_795_478_400), "2026-11-24T00:00:00Z");
    }

    #[test]
    fn only_the_newest_setup_logs_are_kept_and_other_files_stay() {
        let dir = tempfile::tempdir().unwrap();
        for index in 0..13 {
            fs::write(dir.path().join(format!("setup-2026{index:02}.log")), "x").unwrap();
        }
        fs::write(dir.path().join("notes.txt"), "keep").unwrap();
        remove_old_logs(dir.path(), 9);
        let mut names: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names.len(), 10);
        assert_eq!(names[0], "notes.txt");
        assert_eq!(
            names[1], "setup-202604.log",
            "the four oldest logs are gone"
        );
    }

    #[test]
    fn the_tail_starts_at_a_whole_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("setup-x.log");
        fs::write(&path, "first line\nsecond line\nthird\n").unwrap();
        assert_eq!(
            read_tail(&path, 1024).unwrap(),
            "first line\nsecond line\nthird\n"
        );
        assert_eq!(read_tail(&path, 14).unwrap(), "third\n");
        // One line longer than the limit: its end is shown instead of an empty view.
        fs::write(&path, "x".repeat(100)).unwrap();
        assert_eq!(read_tail(&path, 10).unwrap(), "x".repeat(10));
        // A missing file is an error, not an empty log.
        assert!(read_tail(&dir.path().join("missing.log"), 10).is_err());
    }
}

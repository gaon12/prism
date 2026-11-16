//! Remembers which files Prism itself wrote out (Markdown, PDF and HWPX exports of
//! documents and conversations), so they are not mistaken for original sources.
//!
//! Why: an exported summary saved inside a searched folder is indexed again. Without this,
//! the AI could quote Prism's own earlier answer back as if it were evidence, and search
//! would show the same text twice with no hint which one is the original.
//!
//! How: after an export is written, the SHA-256 of the exact bytes is recorded here. The
//! content index already stores the SHA-256 of every indexed file, so "is this chunk from
//! a Prism export?" is a set lookup, with no change to the index format.
//!
//! - Renaming or copying the exported file keeps it recognised (same bytes).
//! - Once the person edits the file, the hash changes and it becomes their own document.
//! - Only hashes are stored, never paths or text.
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

/// Oldest hashes are forgotten past this many exports (about 260 KB on disk).
const MAX_ENTRIES: usize = 4096;

struct Registry {
    /// Where the list is saved. `None` keeps it in memory only (tests, or no data folder).
    file: Option<PathBuf>,
    /// Hex SHA-256 values, oldest first. A short list, so a linear scan is fine.
    hashes: VecDeque<String>,
}

static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();

fn registry() -> &'static Mutex<Registry> {
    REGISTRY.get_or_init(|| {
        Mutex::new(Registry {
            file: None,
            hashes: VecDeque::new(),
        })
    })
}

/// Loads the saved list from `<data folder>/generated-exports.json`. Called once at startup.
/// A missing or damaged file starts an empty list; exports still work.
pub fn init(data_dir: &Path) {
    let file = data_dir.join("generated-exports.json");
    let saved: Vec<String> = std::fs::read(&file)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    let mut registry = registry().lock().unwrap_or_else(|error| error.into_inner());
    registry.file = Some(file);
    for hash in saved.into_iter().filter(|hash| valid(hash)) {
        remember(&mut registry.hashes, hash);
    }
}

/// Records the bytes of a file Prism just wrote. Saving the list may fail (a read-only
/// folder); the export itself already succeeded, so that is only logged.
pub fn record(bytes: &[u8]) {
    let hash = format!("{:x}", Sha256::digest(bytes));
    let mut registry = registry().lock().unwrap_or_else(|error| error.into_inner());
    remember(&mut registry.hashes, hash);
    if let Some(file) = &registry.file {
        let list: Vec<&String> = registry.hashes.iter().collect();
        let saved = serde_json::to_vec(&list)
            .map_err(std::io::Error::other)
            .and_then(|json| crate::atomic_file::write(file, &json));
        if let Err(error) = saved {
            eprintln!("Prism could not save the list of exported files: {error}");
        }
    }
}

/// True when `sha256` (hex, as stored in the content index) is a Prism export.
pub fn contains(sha256: &str) -> bool {
    let registry = registry().lock().unwrap_or_else(|error| error.into_inner());
    registry
        .hashes
        .iter()
        .any(|hash| hash.eq_ignore_ascii_case(sha256))
}

fn valid(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Adds a hash once (moving a repeat to the newest end) and drops the oldest past the cap.
fn remember(hashes: &mut VecDeque<String>, hash: String) {
    let hash = hash.to_ascii_lowercase();
    hashes.retain(|known| *known != hash);
    hashes.push_back(hash);
    while hashes.len() > MAX_ENTRIES {
        hashes.pop_front();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembers_each_hash_once_and_forgets_the_oldest() {
        let mut hashes = VecDeque::new();
        for index in 0..MAX_ENTRIES + 2 {
            remember(&mut hashes, format!("{index:064x}"));
        }
        // A repeat moves to the newest end instead of being stored twice.
        remember(&mut hashes, format!("{:064x}", 5));
        assert_eq!(hashes.len(), MAX_ENTRIES);
        assert!(!hashes.contains(&format!("{:064x}", 0)));
        assert_eq!(hashes.back(), Some(&format!("{:064x}", 5)));
        assert_eq!(
            hashes
                .iter()
                .filter(|hash| **hash == format!("{:064x}", 5))
                .count(),
            1
        );
    }

    #[test]
    fn exported_bytes_are_recognised_by_their_index_hash_and_survive_a_restart() {
        let folder = tempfile::tempdir().unwrap();
        init(folder.path());
        let bytes = "# 요약\n\nPrism이 만든 문서".as_bytes();
        record(bytes);
        let hash = format!("{:x}", Sha256::digest(bytes));
        assert!(contains(&hash));
        assert!(contains(&hash.to_ascii_uppercase()));
        // An edited copy is the person's own document.
        assert!(!contains(&format!("{:x}", Sha256::digest(b"# edited"))));
        // The saved list is plain hashes, read back on the next start.
        let saved: Vec<String> = serde_json::from_slice(
            &std::fs::read(folder.path().join("generated-exports.json")).unwrap(),
        )
        .unwrap();
        assert!(saved.contains(&hash));
        assert!(saved.iter().all(|entry| valid(entry)));
    }
}

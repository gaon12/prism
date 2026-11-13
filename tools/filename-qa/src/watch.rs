// Replays the actual catalog update after a walk, without filesystem I/O.
use super::*;

struct WatchCatalog {
    entries: Entries,
}

include!(concat!(env!("OUT_DIR"), "/watch.rs"));

pub(super) fn roots(path: &std::ffi::OsStr) -> io::Result<Vec<PathBuf>> {
    // A separate private input makes A/B use exactly the same changed paths.
    // Neither these paths nor captured rows are printed by the executable.
    let file = fs::File::open(path)?;
    if file.metadata()?.len() > 64 * 1024 {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let paths: Vec<String> = serde_json::from_reader(file)?;
    if paths.is_empty() || paths.len() > 64 || paths.iter().any(|path| path.is_empty()) {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    Ok(paths.into_iter().map(PathBuf::from).collect())
}

pub(super) fn measure(entries: &mut Entries, paths: &[PathBuf]) -> io::Result<serde_json::Value> {
    if !entries.refs().all(|entry| entry.is_seen(1)) {
        return Err(io::Error::other(
            "Watch replay requires an all-seen catalog",
        ));
    }
    let mut catalog = WatchCatalog {
        entries: std::mem::take(entries),
    };
    let before_count = catalog.entries.len();
    let uncertain = HashSet::new();
    let before_cpu = crate::cpu_clock::ThreadUsage::read()?;
    let started = Instant::now();
    production_update(&mut catalog, paths, 1, &uncertain);
    let wall = started.elapsed();
    let cpu = crate::cpu_clock::ThreadUsage::read()?.since(before_cpu)?;
    *entries = catalog.entries;
    if entries.len() != before_count {
        return Err(io::Error::other(
            "Watch replay changed the all-seen catalog",
        ));
    }
    let normalized: Vec<_> = paths.iter().map(|path| path_key(path)).collect();
    Ok(serde_json::json!({
        "wallSeconds":wall.as_secs_f64(), "threadCpu":cpu,
        "pathCount":paths.len(),
        "normalizedPathsSha256":format!("{:x}", Sha256::digest(serde_json::to_vec(&normalized)?)),
        "productionLoopSha256":format!("{:x}", Sha256::digest(include_bytes!(concat!(env!("OUT_DIR"), "/watch.rs")))),
        "allRowsSeen":true, "sweep":1, "entriesBefore":before_count, "entriesAfter":entries.len(),
        "catalogLockHeld":false, "lockContention":false, "filesystemWalkIncluded":false
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_watch_loop_matches_full_key_reference_with_overlapping_paths() {
        let roots = [
            r"C:\docs",
            r"C:\docs\nested",
            r"C:\outside\single.md",
            r"C:\absent",
        ]
        .map(PathBuf::from);
        for reverse in [false, true] {
            let mut paths = roots.to_vec();
            if reverse {
                paths.reverse();
            }
            let mut catalog = WatchCatalog {
                entries: Entries::new(),
            };
            for (index, (path, seen)) in [
                (r"C:\docs", 1),
                (r"C:\docs\seen.md", 7),
                (r"C:\docs\old.md", 1),
                (r"C:\docs\nested\old.md", 1),
                (r"C:\docs\uncertain.md", 1),
                (r"C:\docs-other\keep.md", 1),
                (r"C:\outside\single.md", 1),
                (r"C:\outside\keep.md", 1),
            ]
            .into_iter()
            .enumerate()
            {
                catalog.entries.insert(&Entry::new(
                    raw_mapper::raw(FileRecord {
                        path: path.into(),
                        is_dir: index == 0,
                        size: index as u64,
                        modified_ms: Some(i64::MIN),
                    }),
                    seen,
                ));
            }
            let uncertain = HashSet::from([path_key(Path::new(r"C:\docs\uncertain.md"))]);
            let keys: Vec<_> = paths.iter().map(|path| path_key(path)).collect();
            let mut scratch = String::new();
            let expected: Vec<_> = catalog
                .entries
                .refs()
                .filter_map(|entry| {
                    let key = entry.write_key(&mut scratch);
                    (entry.is_seen(7)
                        || uncertain.contains(key)
                        || !keys.iter().any(|root| under(key, root)))
                    .then(|| key.to_string())
                })
                .collect();
            production_update(&mut catalog, &paths, 7, &uncertain);
            let actual: Vec<_> = catalog
                .entries
                .refs()
                .map(|entry| entry.write_key(&mut scratch).to_string())
                .collect();
            assert_eq!(actual, expected);
            assert_eq!(actual.len(), 4);
        }
    }
}

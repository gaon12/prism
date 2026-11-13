use sha2::{Digest, Sha256};
use std::io::{self, BufReader, Read, Write};
use std::time::Instant;

#[derive(Clone, Copy, PartialEq, Eq)]
enum StageMode {
    Decode,
    Baseline,
    Stages,
    ReconcileLegacy,
    ReconcileSeen,
}

/// The first whole-drive sweep has seen every inserted row. The legacy path
/// builds each key before checking seen; the current path checks seen first.
/// Count only keys the actual catalog method passes to its caller.
fn reconcile(catalog: &mut Entries, mode: StageMode, sweep: u64) -> u64 {
    let mut keys = 0;
    match mode {
        StageMode::ReconcileLegacy => catalog.retain(|_, entry| {
            keys += 1;
            entry.is_seen(sweep)
        }),
        StageMode::ReconcileSeen => catalog.retain_seen_or(sweep, |_, _| {
            keys += 1;
            false
        }),
        _ => unreachable!("reconcile requires an explicit reconcile mode"),
    }
    keys
}

struct StageBuilder {
    mode: StageMode,
    rows: Vec<FileRecord>,
    raws: Vec<Raw>,
    ready: Vec<Entry>,
    catalog: Entries,
    raw_time: Duration,
    entry_time: Duration,
    insert_time: Duration,
    prepare_time: Duration,
    ascii_rows: u64,
}

impl StageBuilder {
    fn new(mode: StageMode) -> Self {
        Self {
            mode,
            rows: Vec::new(),
            raws: Vec::new(),
            ready: Vec::new(),
            catalog: Entries::new(),
            raw_time: Duration::ZERO,
            entry_time: Duration::ZERO,
            insert_time: Duration::ZERO,
            prepare_time: Duration::ZERO,
            ascii_rows: 0,
        }
    }
    fn push(&mut self, row: FileRecord) {
        self.ascii_rows += u64::from(row.path.is_ascii());
        self.rows.push(row);
        if self.rows.len() == prism_ntfs_protocol::MAX_BATCH_RECORDS {
            self.flush();
        }
    }
    fn flush(&mut self) {
        if self.rows.is_empty() {
            return;
        }
        if self.mode == StageMode::Decode {
            self.rows.clear();
            return;
        }
        let started = Instant::now();
        if self.mode == StageMode::Stages {
            let stage = Instant::now();
            self.raws.extend(self.rows.drain(..).map(raw_mapper::raw));
            self.raw_time += stage.elapsed();
            let stage = Instant::now();
            self.ready
                .extend(self.raws.drain(..).map(|row| Entry::new(row, 1)));
            self.entry_time += stage.elapsed();
        } else {
            // The production worker's fused mapping loop, followed by the same
            // compact insertion. No clocks are sampled per row.
            self.ready.extend(
                self.rows
                    .drain(..)
                    .map(|row| Entry::new(raw_mapper::raw(row), 1)),
            );
        }
        let stage = Instant::now();
        for entry in self.ready.drain(..) {
            self.catalog.insert(&entry);
        }
        self.insert_time += stage.elapsed();
        self.prepare_time += started.elapsed();
    }
}

fn stage_hash_text(hash: &mut Sha256, value: Option<&str>) {
    match value {
        Some(value) => {
            hash.update([1]);
            hash.update((value.len() as u64).to_le_bytes());
            hash.update(value.as_bytes());
        }
        None => hash.update([0]),
    }
}

fn stage_signature(catalog: &Entries) -> String {
    let mut hash = Sha256::new();
    let mut buffer = String::new();
    for entry in catalog.refs() {
        stage_hash_text(&mut hash, Some(entry.write_path(&mut buffer)));
        stage_hash_text(&mut hash, Some(entry.write_key(&mut buffer)));
        if entry.has_search() {
            stage_hash_text(&mut hash, Some(entry.write_search(&mut buffer)));
        } else {
            stage_hash_text(&mut hash, None);
        }
        stage_hash_text(&mut hash, entry.write_initials(&mut buffer));
        stage_hash_text(&mut hash, entry.write_keyboard(&mut buffer));
        hash.update([u8::from(entry.has_name()), u8::from(entry.is_dir())]);
        hash.update(entry.size().to_le_bytes());
        match entry.modified_ms() {
            Some(value) => {
                hash.update([1]);
                hash.update(value.to_le_bytes());
            }
            None => hash.update([0]),
        }
        hash.update(entry.seen().to_le_bytes());
    }
    format!("{:x}", hash.finalize())
}

pub(super) fn run_stages() -> io::Result<()> {
    if !cfg!(windows) {
        return Err(io::ErrorKind::Unsupported.into());
    }
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2 {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let mode = match args[0].to_str() {
        Some("decode") => StageMode::Decode,
        Some("baseline") => StageMode::Baseline,
        Some("stages") => StageMode::Stages,
        Some("reconcile-legacy") => StageMode::ReconcileLegacy,
        Some("reconcile-seen") => StageMode::ReconcileSeen,
        _ => return Err(io::ErrorKind::InvalidInput.into()),
    };
    // The driver must assign the owned process to a verified Job Object before
    // sending G. Capture files are not opened until after that signal.
    println!("{{\"waitingForStart\":true}}");
    io::stdout().flush()?;
    let mut gate = [0];
    io::stdin().read_exact(&mut gate)?;
    if gate != *b"G" || usable_cpus() != 1 {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    let mut builder = StageBuilder::new(mode);
    let started = Instant::now();
    let summary = crate::captured_rows::read(
        BufReader::with_capacity(1024 * 1024, fs::File::open(&args[1])?),
        || Ok(()),
        |row| {
            builder.push(row);
            Ok(())
        },
    )?;
    builder.flush();
    let build_time = started.elapsed();
    let is_reconcile = matches!(mode, StageMode::ReconcileLegacy | StageMode::ReconcileSeen);
    let before_verification = Instant::now();
    let before_signature = is_reconcile.then(|| stage_signature(&builder.catalog));
    let before_verification_time = before_verification.elapsed();
    let reconcile_measurement = if is_reconcile {
        let count = builder.catalog.len();
        let before_cpu = crate::cpu_clock::ThreadUsage::read()?;
        let started = Instant::now();
        let keys = reconcile(&mut builder.catalog, mode, 1);
        let wall = started.elapsed();
        let cpu = crate::cpu_clock::ThreadUsage::read()?.since(before_cpu)?;
        if builder.catalog.len() != count {
            return Err(io::Error::other("Reconcile changed the all-seen catalog"));
        }
        Some(serde_json::json!({
            "wallSeconds":wall.as_secs_f64(),"threadCpu":cpu,
            "keysPassedToPredicate":keys,"entriesBefore":count,"entriesAfter":builder.catalog.len(),
            "allRowsSeen":true,"sweep":1,"catalogLockHeld":false,"lockContention":false
        }))
    } else {
        None
    };
    let verification = Instant::now();
    let signature = (mode != StageMode::Decode).then(|| stage_signature(&builder.catalog));
    let verification_time = verification.elapsed();
    if is_reconcile && before_signature != signature {
        return Err(io::Error::other("Reconcile changed stored catalog fields"));
    }
    println!(
        "{}",
        serde_json::json!({
            "mode":args[0].to_str(),"records":summary.records,"pathBytes":summary.path_bytes,
            "revision":summary.revision,"asciiRows":builder.ascii_rows,
            "batchRecords":prism_ntfs_protocol::MAX_BATCH_RECORDS,
            "catalogEntries":builder.catalog.len(),"catalogSha256":signature,
            "buildWallSeconds":build_time.as_secs_f64(),
            "prepareWallSeconds":builder.prepare_time.as_secs_f64(),
            "rawWallSeconds":(mode==StageMode::Stages).then_some(builder.raw_time.as_secs_f64()),
            "entryWallSeconds":(mode==StageMode::Stages).then_some(builder.entry_time.as_secs_f64()),
            "insertWallSeconds":(mode!=StageMode::Decode).then_some(builder.insert_time.as_secs_f64()),
            "decodeAndDispatchWallSeconds":build_time.saturating_sub(builder.prepare_time).as_secs_f64(),
            "verificationWallSeconds":verification_time.as_secs_f64(),
            "beforeReconcileSha256":before_signature,
            "beforeReconcileVerificationWallSeconds":is_reconcile.then_some(before_verification_time.as_secs_f64()),
            "reconcile":reconcile_measurement,
            "rawMapperSha256":format!("{:x}",Sha256::digest(include_bytes!(concat!(env!("OUT_DIR"),"/raw.rs"))))
        })
    );
    Ok(())
}

#[cfg(test)]
mod stage_tests {
    use super::*;
    #[test]
    fn reconcile_modes_keep_the_same_seen_rows_and_drop_stale_rows() {
        for stale in [false, true] {
            let mut signatures = Vec::new();
            for mode in [StageMode::ReconcileLegacy, StageMode::ReconcileSeen] {
                let mut entries = Entries::new();
                for (index, path) in [r"C:\a\first.md", r"C:\a\문서.md", r"C:\a\last.md"]
                    .into_iter()
                    .enumerate()
                {
                    let seen = if stale && index == 1 { 2 } else { 1 };
                    entries.insert(&Entry::new(
                        raw_mapper::raw(FileRecord {
                            path: path.into(),
                            is_dir: false,
                            size: index as u64,
                            modified_ms: Some(i64::MIN),
                        }),
                        seen,
                    ));
                }
                let keys = reconcile(&mut entries, mode, 1);
                assert_eq!(entries.len(), if stale { 2 } else { 3 });
                assert_eq!(
                    keys,
                    if mode == StageMode::ReconcileLegacy {
                        3
                    } else {
                        u64::from(stale)
                    }
                );
                signatures.push(stage_signature(&entries));
            }
            assert_eq!(signatures[0], signatures[1]);
        }
    }
    #[test]
    fn fused_and_separated_mapping_keep_all_catalog_text_and_metadata_equal() {
        let mut baseline = StageBuilder::new(StageMode::Baseline);
        let mut stages = StageBuilder::new(StageMode::Stages);
        for (index, path) in [
            r"C:\",
            r"C:\Mixed\Report.TXT",
            r"C:\Mixed\보고서.md",
            r"C:\Mixed\가.md",
            r"C:\Mixed\Report.TXT",
        ]
        .into_iter()
        .enumerate()
        {
            let row = FileRecord {
                path: path.into(),
                is_dir: index == 0,
                size: index as u64,
                modified_ms: if index.is_multiple_of(2) {
                    Some(i64::MIN)
                } else {
                    None
                },
            };
            baseline.push(row.clone());
            stages.push(row);
        }
        baseline.flush();
        stages.flush();
        assert_eq!(baseline.catalog.len(), 4);
        assert_eq!(
            stage_signature(&baseline.catalog),
            stage_signature(&stages.catalog)
        );
        assert_eq!(baseline.ascii_rows, stages.ascii_rows);
    }
}

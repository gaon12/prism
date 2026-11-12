//! Compare the live heap of the original filename catalog layout with the
//! compact catalog, on the same entries.
//!
//!   cargo run --release --manifest-path tools/catalog-qa/Cargo.toml \
//!     --example catalog_memory -- (old|compact) [--walk C:\ | --synthetic 2000000]
//!
//! Measure one layout per process to keep the benchmark's own peak low.
//!
//! `--walk` reads real directory names and metadata (never file contents) and
//! does not follow links. It prints only counts, bytes and timings.
use prism_catalog_qa::filename_compact::{Catalog, EntryInput, SearchScratch};
use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

/// Counts live heap bytes exactly, so each layout's cost is measured, not guessed.
struct Counting;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let live = LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
        PEAK.fetch_max(live, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }
    // Count a resize by its size change, as the system allocator may resize in
    // place; the default would count the old and new blocks together.
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if new_size >= layout.size() {
            let live = LIVE.fetch_add(new_size - layout.size(), Ordering::Relaxed)
                + (new_size - layout.size());
            PEAK.fetch_max(live, Ordering::Relaxed);
        } else {
            LIVE.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}
#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// One raw row, as the walker/service hands it to the index.
struct Row {
    path: String,
    is_dir: bool,
    size: u64,
    modified_ms: Option<i64>,
}

/// The original per-entry layout (index.rs `Entry`), for an honest comparison.
#[allow(dead_code)]
struct OldEntry {
    path: Box<str>,
    key: Arc<str>,
    search: Option<Box<str>>,
    name_start: u32,
    initials_path: Option<Box<str>>,
    keyboard_path: Option<Box<str>>,
    is_dir: bool,
    size: u64,
    modified_ms: Option<i64>,
    seen: u64,
}

fn key_of(path: &str) -> String {
    let mut key: String = path
        .chars()
        .map(|ch| if ch == '\\' { '/' } else { ch })
        .collect::<String>()
        .to_lowercase();
    key.truncate(key.trim_end_matches('/').len());
    key
}

fn walk(root: &str) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut stack = vec![std::path::PathBuf::from(root)];
    while let Some(dir) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for item in read.flatten() {
            let Ok(meta) = item.metadata() else { continue };
            let path = item.path().to_string_lossy().into_owned();
            // `DirEntry::metadata` does not traverse links; skip them entirely.
            if meta.file_type().is_symlink() {
                continue;
            }
            let modified_ms = meta
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|age| age.as_millis() as i64);
            if meta.is_dir() {
                stack.push(item.path());
            }
            rows.push(Row {
                path,
                is_dir: meta.is_dir(),
                size: meta.len(),
                modified_ms,
            });
        }
    }
    rows
}

fn synthetic(count: usize) -> Vec<Row> {
    // Roughly a Windows system drive: 8% directories, depth 3-9, mixed case.
    let words = [
        "Windows",
        "Program Files",
        "Users",
        "AppData",
        "Local",
        "Microsoft",
        "Packages",
        "node_modules",
        "src",
        "Cache",
        "Temp",
        "WinSxS",
        "assets",
        "Resources",
        "lib",
    ];
    let mut rows = Vec::with_capacity(count);
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for index in 0..count {
        let depth = 3 + (next() % 7) as usize;
        let mut path = String::from(r"C:");
        for level in 0..depth {
            path.push('\\');
            path.push_str(words[(next() as usize + level) % words.len()]);
            path.push_str(&format!("{}", next() % 40));
        }
        let is_dir = index % 12 == 0;
        if !is_dir {
            path.push_str(&format!(
                r"\File_{index:07}.{}",
                ["dll", "txt", "png", "json"][index % 4]
            ));
        }
        rows.push(Row {
            path,
            is_dir,
            size: index as u64,
            modified_ms: Some(1),
        });
    }
    rows
}

fn mib(bytes: usize) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let layout = if args.is_empty() {
        "compact".to_string()
    } else {
        args.remove(0)
    };
    assert!(
        layout == "old" || layout == "compact",
        "first argument: old or compact"
    );
    let started = Instant::now();
    let rows = match args.first().map(String::as_str) {
        Some("--walk") => walk(args.get(1).map_or(r"C:\", String::as_str)),
        Some("--synthetic") => synthetic(
            args.get(1)
                .and_then(|n| n.parse().ok())
                .unwrap_or(2_000_000),
        ),
        _ => synthetic(2_000_000),
    };
    let path_bytes: usize = rows.iter().map(|row| row.path.len()).sum();
    println!(
        "rows={} averagePathBytes={:.1} collectMs={}",
        rows.len(),
        path_bytes as f64 / rows.len().max(1) as f64,
        started.elapsed().as_millis()
    );

    if layout == "old" {
        measure_old(&rows);
    } else {
        measure_compact(&rows);
    }
}

fn measure_old(rows: &[Row]) {
    // Original layout: BTreeMap<Arc<str>, Entry> with two whole-path strings.
    let base = LIVE.load(Ordering::Relaxed);
    let started = Instant::now();
    let mut old: BTreeMap<Arc<str>, OldEntry> = BTreeMap::new();
    for row in rows {
        let key: Arc<str> = key_of(&row.path).into();
        old.insert(
            key.clone(),
            OldEntry {
                path: row.path.as_str().into(),
                key,
                search: None,
                name_start: 0,
                initials_path: None,
                keyboard_path: None,
                is_dir: row.is_dir,
                size: row.size,
                modified_ms: row.modified_ms,
                seen: 1,
            },
        );
    }
    let old_bytes = LIVE.load(Ordering::Relaxed) - base;
    println!(
        "old layout: heap={:.1} MiB perEntry={:.0} B buildMs={}",
        mib(old_bytes),
        old_bytes as f64 / rows.len() as f64,
        started.elapsed().as_millis()
    );
    drop(old);
}

fn measure_compact(rows: &[Row]) {
    // Compact layout on the same rows.
    let base = LIVE.load(Ordering::Relaxed);
    PEAK.store(base, Ordering::Relaxed);
    let started = Instant::now();
    let mut catalog = Catalog::new();
    let path_bytes = rows.iter().map(|row| row.path.len()).sum();
    catalog.reserve(rows.len(), path_bytes);
    let mut key = String::new();
    for row in rows {
        key.clear();
        key.push_str(&key_of(&row.path));
        // A fresh bulk build: keys are unique, so skip the replace lookup.
        catalog.insert_new(&EntryInput {
            path: &row.path,
            key: &key,
            search: None,
            initials: None,
            keyboard: None,
            has_name: true,
            is_dir: row.is_dir,
            size: row.size,
            modified_ms: row.modified_ms,
            seen: 1,
        });
    }
    catalog.shrink_to_fit();
    let build_ms = started.elapsed().as_millis();
    let new_bytes = LIVE.load(Ordering::Relaxed) - base;
    let peak = PEAK.load(Ordering::Relaxed) - base;
    println!(
        "compact layout: heap={:.1} MiB perEntry={:.0} B peakDuringBuild={:.1} MiB buildMs={} entries={}",
        mib(new_bytes), new_bytes as f64 / rows.len() as f64, mib(peak), build_ms, catalog.len()
    );
    println!(
        "  heapBytes(self-reported)={:.1} MiB",
        mib(catalog.heap_bytes())
    );

    for (part, value) in catalog.breakdown() {
        println!("  {part}={value}");
    }
    // A full single-thread substring scan over the rebuilt search text, the
    // inner loop of a filename query.
    let started = Instant::now();
    let mut scratch = SearchScratch::default();
    let mut hits = 0usize;
    for entry in catalog.iter() {
        if entry.search_text(&mut scratch).contains("readme") {
            hits += 1;
        }
    }
    println!(
        "full scan (1 thread): hits={hits} ms={}",
        started.elapsed().as_millis()
    );

    // Every key must still resolve: identity lookups stay exact.
    let started = Instant::now();
    let missing = rows
        .iter()
        .filter(|row| !catalog.contains_key(&key_of(&row.path)))
        .count();
    println!(
        "key lookups: missing={missing} ms={}",
        started.elapsed().as_millis()
    );
    assert_eq!(missing, 0);
}

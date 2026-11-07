//! Clipboard history: capture, persistent storage, retention, search and restore.
//!
//! Contract: `.dev/COLlAB.md` (Claude 기록). Commands and settings wiring live in the shared files
//! owned by the integrator; this module only exposes `ClipboardService` and a system source.
//! Clipboard contents are never written to logs or error messages.

use crate::filename_pattern::{initial_projection, FilenamePattern};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs,
    io::{BufReader, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use unicode_normalization::UnicodeNormalization;

pub const MAX_TEXT_BYTES: usize = 1 << 20;
pub const MAX_IMAGE_PIXELS: u64 = 40_000_000;
const PREVIEW_CHARS: usize = 300;
const THUMBNAIL_EDGE: u32 = 256;
const POLL_INTERVAL: Duration = Duration::from_millis(300);
const INDEX_VERSION: u32 = 1;

// ---------------------------------------------------------------------------------------------
// Settings and DTOs
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ClipboardSettings {
    pub enabled: bool,
    pub capture_images: bool,
    pub max_items: u32,
    /// 0 keeps unpinned items until another limit applies.
    pub max_age_days: u32,
    pub max_bytes: u64,
    /// False until the first-run notice has been acknowledged in the UI.
    pub onboarded: bool,
}
impl Default for ClipboardSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            capture_images: true,
            max_items: 500,
            max_age_days: 30,
            max_bytes: 256 << 20,
            onboarded: false,
        }
    }
}
impl ClipboardSettings {
    pub fn validate(&self) -> Result<(), String> {
        if !(10..=10_000).contains(&self.max_items) {
            return Err("Clipboard history must keep between 10 and 10000 items.".into());
        }
        if self.max_age_days > 3650 {
            return Err("Clipboard retention must be 3650 days or less.".into());
        }
        if !((16 << 20)..=(8u64 << 30)).contains(&self.max_bytes) {
            return Err("Clipboard storage must be between 16 MiB and 8 GiB.".into());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ClipKind {
    Text,
    Link,
    Image,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipItem {
    pub id: String,
    pub kind: ClipKind,
    pub preview: String,
    pub byte_size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    pub created_ms: i64,
    pub last_copied_ms: i64,
    pub copy_count: u32,
    pub pinned: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipStatus {
    pub enabled: bool,
    pub paused: bool,
    pub count: usize,
    pub unpinned_count: usize,
    pub bytes: u64,
    pub revision: u64,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipListResponse {
    pub items: Vec<ClipItem>,
    pub total: usize,
    pub status: ClipStatus,
}

/// Raw clipboard payload exchanged with a `ClipboardSource`.
#[derive(Clone, PartialEq)]
pub enum ClipContent {
    Text(String),
    Image {
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    },
}
impl std::fmt::Debug for ClipContent {
    // Never print clipboard contents, even in test failures or panics.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Text(text) => write!(f, "Text({} bytes)", text.len()),
            Self::Image { width, height, .. } => write!(f, "Image({width}x{height})"),
        }
    }
}

/// Abstraction over the OS clipboard so tests never touch the user's real clipboard.
pub trait ClipboardSource: Send + 'static {
    /// A cheap value that changes whenever the clipboard changes; `None` if unsupported,
    /// in which case the monitor reads and hashes on every tick.
    fn change_marker(&mut self) -> Option<u64>;
    /// True when the current owner asked monitors/history to skip this content (password managers).
    fn excluded(&mut self) -> bool;
    fn read(&mut self, include_images: bool) -> Result<Option<ClipContent>, String>;
    fn write(&mut self, content: &ClipContent) -> Result<(), String>;
}

// ---------------------------------------------------------------------------------------------
// Persistent store
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredItem {
    #[serde(flatten)]
    item: ClipItem,
    hash: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IndexFile {
    version: u32,
    items: Vec<StoredItem>,
}

struct Store {
    dir: PathBuf,
    items: Vec<StoredItem>,
    /// Normalized full text for text/link items, keyed by id, used for search.
    search_text: HashMap<String, (String, Option<String>)>,
    settings: ClipboardSettings,
    revision: u64,
    last_error: Option<String>,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}
fn normalized(text: &str) -> String {
    text.nfc().collect::<String>().to_lowercase()
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn valid_id(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}
fn is_link(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_whitespace) || trimmed.len() > 8192 {
        return false;
    }
    tauri::Url::parse(trimmed).is_ok_and(|url| match url.scheme() {
        "http" | "https" => url.host_str().is_some(),
        "mailto" => !url.path().is_empty(),
        _ => false,
    })
}
fn content_hash(content: &ClipContent) -> String {
    let mut hasher = Sha256::new();
    match content {
        ClipContent::Text(text) => {
            hasher.update(b"text\0");
            hasher.update(text.as_bytes());
        }
        ClipContent::Image {
            width,
            height,
            rgba,
        } => {
            hasher.update(b"image\0");
            hasher.update(width.to_le_bytes());
            hasher.update(height.to_le_bytes());
            hasher.update(rgba);
        }
    }
    hex(&hasher.finalize())
}
fn write_atomic(path: &Path, contents: &[u8]) -> Result<(), String> {
    let temporary = path.with_extension("pending");
    let mut file = fs::File::create(&temporary).map_err(|error| error.to_string())?;
    file.write_all(contents)
        .and_then(|_| file.sync_all())
        .map_err(|error| error.to_string())?;
    drop(file);
    fs::rename(&temporary, path).map_err(|error| error.to_string())
}

fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let mut output = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut output, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|error| error.to_string())?;
        writer
            .write_image_data(rgba)
            .map_err(|error| error.to_string())?;
    }
    Ok(output)
}
fn decode_png(path: &Path) -> Result<ClipContent, String> {
    let file = fs::File::open(path).map_err(|_| "The stored image is unavailable.".to_string())?;
    let mut decoder = png::Decoder::new(BufReader::new(file));
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info().map_err(|error| error.to_string())?;
    let mut buffer = vec![0; reader.output_buffer_size().ok_or("Image is too large.")?];
    let info = reader
        .next_frame(&mut buffer)
        .map_err(|error| error.to_string())?;
    buffer.truncate(info.buffer_size());
    let rgba = match (info.color_type, info.bit_depth) {
        (png::ColorType::Rgba, png::BitDepth::Eight) => buffer,
        (png::ColorType::Rgb, png::BitDepth::Eight) => buffer
            .chunks_exact(3)
            .flat_map(|pixel| [pixel[0], pixel[1], pixel[2], 255])
            .collect(),
        _ => return Err("Unsupported stored image format.".into()),
    };
    Ok(ClipContent::Image {
        width: info.width,
        height: info.height,
        rgba,
    })
}
/// Box-filter downscale so the longest edge is at most `edge` pixels.
fn thumbnail(width: u32, height: u32, rgba: &[u8], edge: u32) -> (u32, u32, Vec<u8>) {
    let scale = (width.max(height) as f64 / edge as f64).max(1.0);
    let (tw, th) = (
        ((width as f64 / scale).round() as u32).max(1),
        ((height as f64 / scale).round() as u32).max(1),
    );
    if (tw, th) == (width, height) {
        return (width, height, rgba.to_vec());
    }
    let mut output = Vec::with_capacity((tw * th * 4) as usize);
    for y in 0..th {
        let (y0, y1) = (
            (y as u64 * height as u64 / th as u64) as u32,
            (((y + 1) as u64 * height as u64).div_ceil(th as u64) as u32).min(height),
        );
        for x in 0..tw {
            let (x0, x1) = (
                (x as u64 * width as u64 / tw as u64) as u32,
                (((x + 1) as u64 * width as u64).div_ceil(tw as u64) as u32).min(width),
            );
            let mut sum = [0u64; 4];
            let mut count = 0u64;
            for sy in y0..y1.max(y0 + 1) {
                for sx in x0..x1.max(x0 + 1) {
                    let offset = ((sy * width + sx) * 4) as usize;
                    for channel in 0..4 {
                        sum[channel] += rgba[offset + channel] as u64;
                    }
                    count += 1;
                }
            }
            output.extend(sum.map(|value| (value / count) as u8));
        }
    }
    (tw, th, output)
}
fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let value = (chunk[0] as u32) << 16
            | (*chunk.get(1).unwrap_or(&0) as u32) << 8
            | *chunk.get(2).unwrap_or(&0) as u32;
        for index in 0..4 {
            if index <= chunk.len() {
                output.push(TABLE[(value >> (18 - index * 6) & 63) as usize] as char);
            } else {
                output.push('=');
            }
        }
    }
    output
}

impl Store {
    fn open(dir: PathBuf, settings: ClipboardSettings) -> Self {
        let mut store = Self {
            dir,
            items: vec![],
            search_text: HashMap::new(),
            settings,
            revision: 1,
            last_error: None,
        };
        if let Err(error) = store.load() {
            store.last_error = Some(error);
        } else if store.prune(now_ms()) > 0 {
            let _ = store.changed();
        }
        store
    }
    fn blobs(&self) -> PathBuf {
        self.dir.join("blobs")
    }
    fn blob(&self, id: &str, suffix: &str) -> PathBuf {
        self.blobs().join(format!("{id}.{suffix}"))
    }
    fn load(&mut self) -> Result<(), String> {
        fs::create_dir_all(self.blobs())
            .map_err(|error| format!("Clipboard storage unavailable: {error}"))?;
        let index = self.dir.join("index.json");
        let parsed = match fs::read(&index) {
            Ok(bytes) => match serde_json::from_slice::<IndexFile>(&bytes) {
                Ok(file) if file.version == INDEX_VERSION => file.items,
                _ => {
                    // Preserve the damaged file for recovery instead of overwriting it.
                    let backup = self.dir.join(format!("index.corrupt-{}.json", now_ms()));
                    let _ = fs::rename(&index, &backup);
                    return Err(format!(
                        "Clipboard history index was unreadable and was moved to {}.",
                        backup.display()
                    ));
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => vec![],
            Err(error) => return Err(format!("Could not read clipboard history: {error}")),
        };
        let mut seen = std::collections::HashSet::new();
        for stored in parsed {
            let id = stored.item.id.clone();
            if !valid_id(&id) || stored.hash != id || !seen.insert(id.clone()) {
                continue;
            }
            match stored.item.kind {
                ClipKind::Image => {
                    if !self.blob(&id, "png").is_file() {
                        continue;
                    }
                }
                _ => match fs::read_to_string(self.blob(&id, "txt")) {
                    Ok(text) => self.index_text(&id, &text),
                    Err(_) => continue,
                },
            }
            self.items.push(stored);
        }
        self.sort();
        self.remove_orphans();
        Ok(())
    }
    fn remove_orphans(&self) {
        let Ok(entries) = fs::read_dir(self.blobs()) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let id = name.split('.').next().unwrap_or_default();
            if !self.items.iter().any(|stored| stored.item.id == id) {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
    fn index_text(&mut self, id: &str, text: &str) {
        let normalized = normalized(text);
        let projection = initial_projection(&normalized);
        self.search_text
            .insert(id.to_string(), (normalized, projection));
    }
    fn sort(&mut self) {
        self.items.sort_by(|a, b| {
            b.item
                .pinned
                .cmp(&a.item.pinned)
                .then(b.item.last_copied_ms.cmp(&a.item.last_copied_ms))
                .then(a.item.id.cmp(&b.item.id))
        });
    }
    fn save(&mut self) -> Result<(), String> {
        let file = IndexFile {
            version: INDEX_VERSION,
            items: self.items.clone(),
        };
        let bytes = serde_json::to_vec(&file).map_err(|error| error.to_string())?;
        write_atomic(&self.dir.join("index.json"), &bytes)
            .map_err(|error| format!("Could not save clipboard history: {error}"))
    }
    fn bytes(&self) -> u64 {
        self.items.iter().map(|stored| stored.item.byte_size).sum()
    }
    fn status(&self, paused: bool) -> ClipStatus {
        ClipStatus {
            enabled: self.settings.enabled,
            paused,
            count: self.items.len(),
            unpinned_count: self
                .items
                .iter()
                .filter(|stored| !stored.item.pinned)
                .count(),
            bytes: self.bytes(),
            revision: self.revision,
            last_error: self.last_error.clone(),
        }
    }
    fn changed(&mut self) -> Result<(), String> {
        self.revision += 1;
        let result = self.save();
        self.last_error = result.as_ref().err().cloned();
        result
    }
    fn report_error(&mut self, message: &str) -> bool {
        if self.last_error.as_deref() == Some(message) {
            return false;
        }
        self.last_error = Some(message.into());
        self.revision += 1;
        true
    }

    /// Adds content or moves an identical existing item to the top. Returns false if ignored.
    fn add(&mut self, content: &ClipContent, at: i64, count_copy: bool) -> Result<bool, String> {
        match content {
            ClipContent::Text(text)
                if text.trim().is_empty() || text.len() > MAX_TEXT_BYTES || text.contains('\0') =>
            {
                return Ok(false)
            }
            ClipContent::Image {
                width,
                height,
                rgba,
            } if *width == 0
                || *height == 0
                || *width as u64 * *height as u64 > MAX_IMAGE_PIXELS
                || rgba.len() as u64 != *width as u64 * *height as u64 * 4 =>
            {
                return Ok(false)
            }
            _ => {}
        }
        // The newest copy must sort first. The clock has millisecond resolution, so two copies
        // in the same millisecond would tie and fall back to id order (seen on fast CI runners).
        // Keep each new timestamp at least 1 ms after the newest one already stored.
        let at = self
            .items
            .iter()
            .map(|stored| stored.item.last_copied_ms)
            .max()
            .map_or(at, |latest| at.max(latest.saturating_add(1)));
        let hash = content_hash(content);
        if let Some(existing) = self.items.iter_mut().find(|stored| stored.hash == hash) {
            existing.item.last_copied_ms = at;
            if count_copy {
                existing.item.copy_count = existing.item.copy_count.saturating_add(1);
            }
            self.sort();
            self.changed()?;
            return Ok(true);
        }
        let item = match content {
            ClipContent::Text(text) => {
                write_atomic(&self.blob(&hash, "txt"), text.as_bytes())?;
                self.index_text(&hash, text);
                let flat: String = text
                    .trim()
                    .chars()
                    .map(|ch| if ch.is_control() { ' ' } else { ch })
                    .take(PREVIEW_CHARS)
                    .collect();
                ClipItem {
                    id: hash.clone(),
                    kind: if is_link(text) {
                        ClipKind::Link
                    } else {
                        ClipKind::Text
                    },
                    preview: flat,
                    byte_size: text.len() as u64,
                    width: None,
                    height: None,
                    created_ms: at,
                    last_copied_ms: at,
                    copy_count: 1,
                    pinned: false,
                }
            }
            ClipContent::Image {
                width,
                height,
                rgba,
            } => {
                let png = encode_png(*width, *height, rgba)?;
                let (tw, th, small) = thumbnail(*width, *height, rgba, THUMBNAIL_EDGE);
                let thumb = encode_png(tw, th, &small)?;
                write_atomic(&self.blob(&hash, "png"), &png)?;
                write_atomic(&self.blob(&hash, "thumb.png"), &thumb)?;
                ClipItem {
                    id: hash.clone(),
                    kind: ClipKind::Image,
                    preview: format!("{width}×{height}"),
                    byte_size: (png.len() + thumb.len()) as u64,
                    width: Some(*width),
                    height: Some(*height),
                    created_ms: at,
                    last_copied_ms: at,
                    copy_count: 1,
                    pinned: false,
                }
            }
        };
        self.items.push(StoredItem { item, hash });
        self.sort();
        self.prune(at);
        self.changed()?;
        Ok(true)
    }
    fn remove_files(&mut self, id: &str) {
        for suffix in ["txt", "png", "thumb.png"] {
            let _ = fs::remove_file(self.blob(id, suffix));
        }
        self.search_text.remove(id);
    }
    /// Retention: age, then count, then disk size; pinned items are never pruned.
    fn prune(&mut self, at: i64) -> usize {
        let mut removed = vec![];
        if self.settings.max_age_days > 0 {
            let cutoff = at - self.settings.max_age_days as i64 * 86_400_000;
            self.items.retain(|stored| {
                let keep = stored.item.pinned || stored.item.last_copied_ms >= cutoff;
                if !keep {
                    removed.push(stored.item.id.clone());
                }
                keep
            });
        }
        // Items are sorted pinned-first, newest-first, so the oldest unpinned item is the last one.
        let unpinned = |items: &Vec<StoredItem>| items.iter().filter(|s| !s.item.pinned).count();
        while unpinned(&self.items) > self.settings.max_items as usize
            || (self.bytes() > self.settings.max_bytes && unpinned(&self.items) > 0)
        {
            let Some(position) = self.items.iter().rposition(|stored| !stored.item.pinned) else {
                break;
            };
            removed.push(self.items.remove(position).item.id);
        }
        for id in &removed {
            self.remove_files(id);
        }
        removed.len()
    }
    fn find(&self, id: &str) -> Result<&StoredItem, String> {
        self.items
            .iter()
            .find(|stored| stored.item.id == id)
            .ok_or_else(|| "This clipboard item no longer exists.".into())
    }
    fn list(&self, query: &str, kind: Option<ClipKind>, limit: usize) -> (Vec<ClipItem>, usize) {
        let terms: Vec<_> = query
            .split_whitespace()
            .map(|term| FilenamePattern::new(normalized(term)))
            .collect();
        let mut total = 0;
        let mut items = vec![];
        for stored in &self.items {
            if kind.is_some_and(|kind| kind != stored.item.kind) {
                continue;
            }
            if !terms.is_empty() {
                let Some((text, projection)) = self.search_text.get(&stored.item.id) else {
                    continue; // Images are listed by kind only; they have no searchable text.
                };
                if !terms
                    .iter()
                    .all(|term| term.matches(text, projection.as_deref()))
                {
                    continue;
                }
            }
            total += 1;
            if items.len() < limit {
                items.push(stored.item.clone());
            }
        }
        (items, total)
    }
}

// ---------------------------------------------------------------------------------------------
// Service and monitor
// ---------------------------------------------------------------------------------------------

type Listener = Arc<dyn Fn(ClipStatus) + Send + Sync>;

/// Monitor state carried between ticks.
struct PollState {
    marker: Option<u64>,
    /// Hash of the last payload seen; only used by sources without change markers.
    last_hash: Option<String>,
    failures: u32,
    /// The next successful read (including empty) establishes the baseline after startup/resume.
    rebaseline: bool,
}
impl PollState {
    /// Content already on the clipboard at startup was copied before Prism was watching.
    fn new(source: &mut dyn ClipboardSource) -> Self {
        let marker = source.change_marker();
        Self {
            marker,
            last_hash: None,
            failures: 0,
            rebaseline: marker.is_none(),
        }
    }
}

/// Identifies Prism's own clipboard write. The OS may re-encode what it reads back (e.g. an
/// image round-tripped through DIB loses exact RGBA), so the change marker observed right after
/// the write is the primary signal and the content hash is the fallback.
#[derive(Clone, Debug, PartialEq)]
struct OwnWrite {
    hash: String,
    marker: Option<u64>,
}

#[derive(Clone)]
pub struct ClipboardService {
    store: Arc<Mutex<Store>>,
    paused: Arc<AtomicBool>,
    /// Content this app wrote itself; the monitor must not count it as a new copy.
    suppressed: Arc<Mutex<Option<OwnWrite>>>,
    writer: Arc<Mutex<Option<Box<dyn ClipboardSource>>>>,
    listener: Listener,
    lifetime: Arc<()>,
}

impl ClipboardService {
    pub fn open(
        dir: PathBuf,
        settings: ClipboardSettings,
        listener: impl Fn(ClipStatus) + Send + Sync + 'static,
    ) -> Self {
        Self {
            store: Arc::new(Mutex::new(Store::open(dir, settings))),
            paused: Arc::new(AtomicBool::new(false)),
            suppressed: Arc::new(Mutex::new(None)),
            writer: Arc::new(Mutex::new(None)),
            listener: Arc::new(listener),
            lifetime: Arc::new(()),
        }
    }
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Store>, String> {
        self.store
            .lock()
            .map_err(|_| "Clipboard history is unavailable.".into())
    }
    fn notify(&self) {
        if let Ok(status) = self.status() {
            (self.listener)(status);
        }
    }
    pub fn status(&self) -> Result<ClipStatus, String> {
        Ok(self.lock()?.status(self.paused.load(Ordering::SeqCst)))
    }
    pub fn apply_settings(&self, settings: ClipboardSettings) -> Result<(), String> {
        settings.validate()?;
        let result = {
            let mut store = self.lock()?;
            store.settings = settings;
            if store.prune(now_ms()) > 0 {
                store.changed()
            } else {
                store.revision += 1;
                Ok(())
            }
        };
        self.notify();
        result
    }
    pub fn set_paused(&self, paused: bool) -> Result<ClipStatus, String> {
        self.paused.store(paused, Ordering::SeqCst);
        self.lock()?.revision += 1;
        self.notify();
        self.status()
    }
    pub fn list(
        &self,
        query: &str,
        kind: Option<ClipKind>,
        limit: usize,
    ) -> Result<ClipListResponse, String> {
        if query.len() > 4096 {
            return Err("Search input is too long (4096 bytes maximum).".into());
        }
        let store = self.lock()?;
        let (items, total) = store.list(query, kind, limit.clamp(1, 1000));
        Ok(ClipListResponse {
            items,
            total,
            status: store.status(self.paused.load(Ordering::SeqCst)),
        })
    }
    pub fn text(&self, id: &str) -> Result<String, String> {
        let store = self.lock()?;
        let stored = store.find(id)?;
        if stored.item.kind == ClipKind::Image {
            return Err("This clipboard item is an image.".into());
        }
        fs::read_to_string(store.blob(id, "txt"))
            .map_err(|_| "The stored text is unavailable.".into())
    }
    pub fn thumbnail_data_url(&self, id: &str) -> Result<String, String> {
        let store = self.lock()?;
        if store.find(id)?.item.kind != ClipKind::Image {
            return Err("This clipboard item is not an image.".into());
        }
        let bytes = fs::read(store.blob(id, "thumb.png"))
            .map_err(|_| "The stored image is unavailable.".to_string())?;
        Ok(format!("data:image/png;base64,{}", base64(&bytes)))
    }
    pub fn set_pinned(&self, id: &str, pinned: bool) -> Result<ClipItem, String> {
        let item = {
            let mut store = self.lock()?;
            let index = store
                .items
                .iter()
                .position(|stored| stored.item.id == id)
                .ok_or("This clipboard item no longer exists.")?;
            store.items[index].item.pinned = pinned;
            let item = store.items[index].item.clone();
            store.sort();
            if !pinned {
                store.prune(now_ms());
            }
            store.changed()?;
            item
        };
        self.notify();
        Ok(item)
    }
    pub fn delete(&self, id: &str) -> Result<(), String> {
        {
            let mut store = self.lock()?;
            store.find(id)?;
            store.items.retain(|stored| stored.item.id != id);
            store.remove_files(id);
            store.changed()?;
        }
        self.notify();
        Ok(())
    }
    pub fn clear(&self, include_pinned: bool) -> Result<(), String> {
        {
            let mut store = self.lock()?;
            let removed: Vec<_> = store
                .items
                .iter()
                .filter(|stored| include_pinned || !stored.item.pinned)
                .map(|stored| stored.item.id.clone())
                .collect();
            store
                .items
                .retain(|stored| !include_pinned && stored.item.pinned);
            for id in &removed {
                store.remove_files(id);
            }
            store.changed()?;
        }
        self.notify();
        Ok(())
    }
    /// Writes a stored item back to the system clipboard (the paste step is the caller's job).
    pub fn copy_to_clipboard(&self, id: &str) -> Result<(), String> {
        let content = {
            let store = self.lock()?;
            let stored = store.find(id)?;
            match stored.item.kind {
                ClipKind::Image => decode_png(&store.blob(id, "png"))?,
                _ => ClipContent::Text(
                    fs::read_to_string(store.blob(id, "txt"))
                        .map_err(|_| "The stored text is unavailable.".to_string())?,
                ),
            }
        };
        self.write_content(content.clone())?;
        {
            let mut store = self.lock()?;
            store.add(&content, now_ms(), false)?;
        }
        self.notify();
        Ok(())
    }

    /// All Prism writes share the injected writer, including calculator copies in isolated tests.
    pub fn write_content(&self, content: ClipContent) -> Result<(), String> {
        let hash = content_hash(&content);
        // Set before writing so a monitor tick racing the write still recognizes it by hash.
        *self
            .suppressed
            .lock()
            .map_err(|_| "Clipboard history is unavailable.")? = Some(OwnWrite {
            hash: hash.clone(),
            marker: None,
        });
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| "Clipboard history is unavailable.")?;
        let writer = writer
            .as_mut()
            .ok_or("Clipboard monitoring has not started.")?;
        let result = writer.write(&content);
        let marker = result.is_ok().then(|| writer.change_marker()).flatten();
        if let Ok(mut slot) = self.suppressed.lock() {
            *slot = result.is_ok().then_some(OwnWrite { hash, marker });
        }
        result
    }

    /// Starts the background monitor. `writer` is a second source handle used for restore writes.
    pub fn start(&self, source: Box<dyn ClipboardSource>, writer: Box<dyn ClipboardSource>) {
        if let Ok(mut slot) = self.writer.lock() {
            *slot = Some(writer);
        }
        let service = self.clone();
        std::thread::spawn(move || service.monitor(source));
    }
    fn monitor(&self, mut source: Box<dyn ClipboardSource>) {
        let mut state = PollState::new(&mut *source);
        let mut last_prune = now_ms();
        loop {
            std::thread::sleep(POLL_INTERVAL);
            if Arc::strong_count(&self.lifetime) == 1 {
                return;
            }
            if self.poll_once(&mut *source, &mut state) {
                self.notify();
            }
            if now_ms() - last_prune > 3_600_000 {
                last_prune = now_ms();
                if let Ok(mut store) = self.store.lock() {
                    if store.prune(last_prune) > 0 {
                        let _ = store.changed();
                    }
                }
            }
        }
    }
    /// One monitor tick. Returns true when history or diagnostic status changed.
    fn poll_once(&self, source: &mut dyn ClipboardSource, state: &mut PollState) -> bool {
        let PollState {
            marker,
            last_hash,
            failures,
            rebaseline,
        } = state;
        let next = source.change_marker();
        if next.is_some() && next == *marker {
            return false;
        }
        let (enabled, images) = match self.store.lock() {
            Ok(store) => (store.settings.enabled, store.settings.capture_images),
            Err(_) => return false,
        };
        if !enabled || self.paused.load(Ordering::SeqCst) {
            // Anything copied while paused or disabled is never recorded, even after resuming.
            // Without change markers the first payload seen after resuming becomes the new
            // baseline instead of a new copy.
            *marker = next;
            *rebaseline = next.is_none();
            return false;
        }
        if source.excluded() {
            *marker = next;
            return false;
        }
        let content = match source.read(images) {
            Ok(content) => content,
            Err(_) => {
                // Another app may hold the clipboard open; retry a few ticks before skipping.
                *failures += 1;
                if *failures >= 5 {
                    *failures = 0;
                    *marker = next;
                    if let Ok(mut store) = self.store.lock() {
                        return store.report_error("The clipboard could not be read.");
                    }
                }
                return false;
            }
        };
        *failures = 0;
        *marker = next;
        let Some(content) = content else {
            // Empty is a valid snapshot, not a request to ignore the next future copy.
            // It also separates identical copies on platforms without change markers.
            *rebaseline = false;
            *last_hash = None;
            return false;
        };
        let hash = content_hash(&content);
        if std::mem::take(rebaseline) {
            *last_hash = Some(hash);
            return false;
        }
        if next.is_none() && last_hash.as_ref() == Some(&hash) {
            return false;
        }
        *last_hash = Some(hash.clone());
        // The first payload observed after a write consumes the record either way, so a stale
        // record can never hide a later copy. With markers on both sides only an exact marker
        // match counts (a user re-copying identical content gets a new marker and is recorded);
        // the hash is the fallback when the marker is unknown.
        let own_write = self
            .suppressed
            .lock()
            .ok()
            .and_then(|mut slot| slot.take())
            .is_some_and(|own| match (own.marker, next) {
                (Some(own), Some(next)) => own == next,
                _ => own.hash == hash,
            });
        if own_write {
            return false;
        }
        match self.store.lock() {
            Ok(mut store) => match store.add(&content, now_ms(), true) {
                Ok(added) => added,
                Err(_) => store.report_error("Clipboard history could not be saved. Check available disk space and permissions."),
            },
            Err(_) => false,
        }
    }
}

impl ClipboardService {
    /// Records synthetic content exactly as if it had been copied (isolated test runs only:
    /// the caller must gate this behind `PRISM_DEV_DATA_DIR` / debug builds).
    pub fn capture(&self, content: ClipContent) -> Result<bool, String> {
        let added = self.lock()?.add(&content, now_ms(), true)?;
        if added {
            self.notify();
        }
        Ok(added)
    }
}

// ---------------------------------------------------------------------------------------------
// In-memory clipboard (isolated runs and tests; never touches the OS clipboard)
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
struct MemoryState {
    marker: u64,
    /// Simulates an OS without clipboard sequence numbers.
    markerless: bool,
    content: Option<ClipContent>,
    excluded: bool,
    fail_reads: u32,
}
/// Shared handle to a fake clipboard. Clone it, pass one clone to `start`, keep one to inject.
#[derive(Clone, Default)]
pub struct MemoryClipboard(Arc<Mutex<MemoryState>>);
impl MemoryClipboard {
    /// A fixture that reports no change markers, like platforms without sequence numbers.
    pub fn without_markers() -> Self {
        let clipboard = Self::default();
        if let Ok(mut state) = clipboard.0.lock() {
            state.markerless = true;
        }
        clipboard
    }
    /// Simulates another app copying `content`.
    pub fn copy(&self, content: ClipContent) {
        if let Ok(mut state) = self.0.lock() {
            state.marker += 1;
            state.content = Some(content);
        }
    }
    /// Simulates a password manager marking the current content as excluded from history.
    pub fn set_excluded(&self, excluded: bool) {
        if let Ok(mut state) = self.0.lock() {
            state.excluded = excluded;
        }
    }
    pub fn content(&self) -> Option<ClipContent> {
        self.0.lock().ok().and_then(|state| state.content.clone())
    }
    #[cfg(test)]
    fn fail_next_reads(&self, count: u32) {
        self.0.lock().unwrap().fail_reads = count;
    }
    #[cfg(test)]
    fn clear(&self) {
        let mut state = self.0.lock().unwrap();
        state.marker += 1;
        state.content = None;
    }
    pub fn boxed(&self) -> Box<dyn ClipboardSource> {
        Box::new(self.clone())
    }
}
impl ClipboardSource for MemoryClipboard {
    fn change_marker(&mut self) -> Option<u64> {
        self.0
            .lock()
            .ok()
            .filter(|state| !state.markerless)
            .map(|state| state.marker)
    }
    fn excluded(&mut self) -> bool {
        self.0.lock().is_ok_and(|state| state.excluded)
    }
    fn read(&mut self, include_images: bool) -> Result<Option<ClipContent>, String> {
        let mut state = self
            .0
            .lock()
            .map_err(|_| "Clipboard fixture unavailable.")?;
        if state.fail_reads > 0 {
            state.fail_reads -= 1;
            return Err("The clipboard is busy.".into());
        }
        Ok(state
            .content
            .clone()
            .filter(|content| include_images || matches!(content, ClipContent::Text(_))))
    }
    fn write(&mut self, content: &ClipContent) -> Result<(), String> {
        self.copy(content.clone());
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// System clipboard source
// ---------------------------------------------------------------------------------------------

pub struct SystemSource<R: tauri::Runtime> {
    app: tauri::AppHandle<R>,
}
pub fn system_source<R: tauri::Runtime>(app: tauri::AppHandle<R>) -> Box<dyn ClipboardSource> {
    Box::new(SystemSource { app })
}

fn missing_clipboard_format(error: &tauri_plugin_clipboard_manager::Error) -> bool {
    // The plugin wraps arboard errors as strings. Compare only the exact error from
    // the locked arboard version: occupied/conversion/unknown failures are not empty.
    matches!(error, tauri_plugin_clipboard_manager::Error::Clipboard(message)
        if message == &arboard::Error::ContentNotAvailable.to_string())
}

impl<R: tauri::Runtime> ClipboardSource for SystemSource<R> {
    fn change_marker(&mut self) -> Option<u64> {
        #[cfg(windows)]
        {
            // SAFETY: no arguments; returns 0 when the window station has no access.
            let value =
                unsafe { windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber() };
            (value != 0).then_some(value as u64)
        }
        #[cfg(not(windows))]
        {
            None
        }
    }
    fn excluded(&mut self) -> bool {
        #[cfg(windows)]
        {
            windows_exclusion::excluded()
        }
        #[cfg(not(windows))]
        {
            false
        }
    }
    fn read(&mut self, include_images: bool) -> Result<Option<ClipContent>, String> {
        use tauri_plugin_clipboard_manager::ClipboardExt;
        // Only explicit format absence is empty. Advertised formats and other read failures
        // stay retryable errors, so a transient failure cannot consume the startup baseline.
        #[cfg(windows)]
        let (has_text, has_image) = windows_exclusion::available_formats();
        #[cfg(not(windows))]
        let (has_text, has_image) = (false, false);
        let clipboard = self.app.clipboard();
        match clipboard.read_text() {
            Ok(text) if !text.is_empty() => return Ok(Some(ClipContent::Text(text))),
            Ok(_) => {}
            Err(error) if !has_text && missing_clipboard_format(&error) => {}
            Err(_) => return Err("The clipboard is busy.".into()),
        }
        if include_images {
            match clipboard.read_image() {
                Ok(image) => {
                    return Ok(Some(ClipContent::Image {
                        width: image.width(),
                        height: image.height(),
                        rgba: image.rgba().to_vec(),
                    }))
                }
                Err(error) if !has_image && missing_clipboard_format(&error) => {}
                Err(_) => return Err("The clipboard is busy.".into()),
            }
        }
        Ok(None)
    }
    fn write(&mut self, content: &ClipContent) -> Result<(), String> {
        use tauri_plugin_clipboard_manager::ClipboardExt;
        let clipboard = self.app.clipboard();
        match content {
            ClipContent::Text(text) => clipboard.write_text(text.clone()),
            ClipContent::Image {
                width,
                height,
                rgba,
            } => clipboard.write_image(&tauri::image::Image::new(rgba, *width, *height)),
        }
        .map_err(|error| error.to_string())
    }
}

#[cfg(windows)]
mod windows_exclusion {
    use windows_sys::Win32::System::{
        DataExchange::{
            CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
            RegisterClipboardFormatW,
        },
        Memory::{GlobalLock, GlobalSize, GlobalUnlock},
    };
    /// Whether text and image formats are advertised. Needs no clipboard lock.
    pub fn available_formats() -> (bool, bool) {
        const CF_BITMAP: u32 = 2;
        const CF_DIB: u32 = 8;
        const CF_UNICODETEXT: u32 = 13;
        const CF_DIBV5: u32 = 17;
        let png = format("PNG");
        // SAFETY: format ids are constants or come from RegisterClipboardFormatW.
        unsafe {
            let text = IsClipboardFormatAvailable(CF_UNICODETEXT) != 0;
            let image = [CF_DIBV5, CF_DIB, CF_BITMAP]
                .into_iter()
                .chain((png != 0).then_some(png))
                .any(|id| IsClipboardFormatAvailable(id) != 0);
            (text, image)
        }
    }
    fn format(name: &str) -> u32 {
        let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        // SAFETY: `wide` is a NUL-terminated UTF-16 string that outlives the call.
        unsafe { RegisterClipboardFormatW(wide.as_ptr()) }
    }
    /// Honors the documented signals password managers set to keep secrets out of history.
    /// https://learn.microsoft.com/windows/win32/dataxchg/clipboard-formats#cloud-clipboard-and-clipboard-history-formats
    pub fn excluded() -> bool {
        // SAFETY: format ids come from RegisterClipboardFormatW; availability checks need no lock.
        unsafe {
            for name in [
                "ExcludeClipboardContentFromMonitorProcessing",
                "Clipboard Viewer Ignore",
            ] {
                let id = format(name);
                if id != 0 && IsClipboardFormatAvailable(id) != 0 {
                    return true;
                }
            }
            let id = format("CanIncludeInClipboardHistory");
            if id == 0 || IsClipboardFormatAvailable(id) == 0 {
                return false;
            }
            if OpenClipboard(std::ptr::null_mut()) == 0 {
                return true; // Cannot verify the opt-out value; err on the side of not recording.
            }
            let handle = GetClipboardData(id);
            let mut allow = false;
            if !handle.is_null() && GlobalSize(handle) >= 4 {
                let pointer = GlobalLock(handle) as *const u32;
                if !pointer.is_null() {
                    allow = std::ptr::read_unaligned(pointer) != 0;
                    GlobalUnlock(handle);
                }
            }
            CloseClipboard();
            !allow
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Tests (isolated fixture source and temporary directories only)
// ---------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    struct Harness {
        _dir: tempfile::TempDir,
        path: PathBuf,
        service: ClipboardService,
        clipboard: MemoryClipboard,
        source: Box<dyn ClipboardSource>,
        state: PollState,
        events: Arc<Mutex<u32>>,
    }
    impl Harness {
        fn new(settings: ClipboardSettings) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("clipboard");
            Self::with_dir(dir, path, settings)
        }
        fn with_dir(dir: tempfile::TempDir, path: PathBuf, settings: ClipboardSettings) -> Self {
            Self::with_clipboard(dir, path, settings, MemoryClipboard::default())
        }
        fn with_clipboard(
            dir: tempfile::TempDir,
            path: PathBuf,
            settings: ClipboardSettings,
            clipboard: MemoryClipboard,
        ) -> Self {
            let events = Arc::new(Mutex::new(0));
            let counter = events.clone();
            let service = ClipboardService::open(path.clone(), settings, move |_| {
                *counter.lock().unwrap() += 1;
            });
            *service.writer.lock().unwrap() = Some(Box::new(clipboard.clone()));
            let mut source: Box<dyn ClipboardSource> = Box::new(clipboard.clone());
            let state = PollState::new(&mut *source);
            Self {
                _dir: dir,
                path,
                service,
                clipboard,
                source,
                state,
                events,
            }
        }
        fn tick(&mut self) -> bool {
            self.service.poll_once(&mut *self.source, &mut self.state)
        }
        fn copy(&mut self, content: ClipContent) -> bool {
            self.clipboard.copy(content);
            self.tick()
        }
        fn copy_text(&mut self, text: &str) -> bool {
            self.copy(ClipContent::Text(text.into()))
        }
        fn previews(&self, query: &str) -> Vec<String> {
            self.service
                .list(query, None, 100)
                .unwrap()
                .items
                .into_iter()
                .map(|item| item.preview)
                .collect()
        }
    }
    fn image(width: u32, height: u32, seed: u8) -> ClipContent {
        ClipContent::Image {
            width,
            height,
            rgba: (0..width * height * 4)
                .map(|value| (value as u8).wrapping_add(seed))
                .collect(),
        }
    }

    /// Writes like an OS that re-encodes images: what is read back differs from what was written.
    struct LossyWriter(MemoryClipboard);
    impl ClipboardSource for LossyWriter {
        fn change_marker(&mut self) -> Option<u64> {
            self.0.change_marker()
        }
        fn excluded(&mut self) -> bool {
            false
        }
        fn read(&mut self, images: bool) -> Result<Option<ClipContent>, String> {
            self.0.read(images)
        }
        fn write(&mut self, content: &ClipContent) -> Result<(), String> {
            let mut altered = content.clone();
            if let ClipContent::Image { rgba, .. } = &mut altered {
                rgba.iter_mut()
                    .skip(3)
                    .step_by(4)
                    .for_each(|alpha| *alpha = 255);
            }
            self.0.copy(altered);
            Ok(())
        }
    }

    #[test]
    fn restored_images_that_the_os_re_encodes_are_not_duplicated() {
        let mut h = Harness::new(ClipboardSettings::default());
        h.copy(image(16, 16, 11));
        let id = h.service.list("", None, 1).unwrap().items[0].id.clone();
        *h.service.writer.lock().unwrap() = Some(Box::new(LossyWriter(h.clipboard.clone())));
        h.service.copy_to_clipboard(&id).unwrap();
        assert_ne!(
            h.clipboard.content(),
            Some(image(16, 16, 11)),
            "fixture re-encoded"
        );
        assert!(!h.tick(), "own write is recognized by its change marker");
        assert_eq!(h.service.status().unwrap().count, 1);
        // A later real copy of other content is still recorded.
        assert!(h.copy_text("after restore"));
        assert_eq!(h.service.status().unwrap().count, 2);
    }

    #[test]
    fn own_write_records_are_consumed_and_do_not_hide_real_copies() {
        // Own write, then another app copies before the monitor ticks, then the user re-copies
        // the restored text: both real copies are recorded.
        let mut h = Harness::new(ClipboardSettings::default());
        h.copy_text("restored");
        let id = h.service.list("", None, 1).unwrap().items[0].id.clone();
        h.service.copy_to_clipboard(&id).unwrap();
        h.clipboard.copy(ClipContent::Text("other app".into()));
        assert!(h.tick(), "content copied after our write is recorded");
        assert!(
            h.copy_text("restored"),
            "a later identical copy is not swallowed"
        );
        let items = h.service.list("", None, 10).unwrap().items;
        assert_eq!(
            (items[0].preview.as_str(), items[0].copy_count),
            ("restored", 2)
        );
        // Own write, then the user immediately copies the same content again (new marker).
        let mut h = Harness::new(ClipboardSettings::default());
        h.copy_text("same");
        let id = h.service.list("", None, 1).unwrap().items[0].id.clone();
        h.service.copy_to_clipboard(&id).unwrap();
        h.clipboard.copy(ClipContent::Text("same".into()));
        assert!(h.tick(), "the user's own re-copy counts");
        assert_eq!(h.service.list("", None, 1).unwrap().items[0].copy_count, 2);
        // Without markers the content hash decides, and the record is still single-use.
        assert!(!h.service.suppressed.lock().unwrap().is_some());
    }

    #[test]
    fn markerless_sources_rebaseline_after_pause_and_disable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("clipboard");
        let clipboard = MemoryClipboard::without_markers();
        clipboard.copy(ClipContent::Text("before start".into()));
        let mut h = Harness::with_clipboard(dir, path, ClipboardSettings::default(), clipboard);
        assert!(!h.tick(), "content present at start is a baseline");
        assert!(!h.tick(), "unchanged content is not re-recorded");
        assert!(h.copy_text("first"));
        h.service.set_paused(true).unwrap();
        assert!(!h.copy_text("copied while paused"));
        h.service.set_paused(false).unwrap();
        assert!(!h.tick(), "paused content is not recorded after resuming");
        assert!(h.copy_text("after resume"));
        h.service
            .apply_settings(ClipboardSettings {
                enabled: false,
                ..Default::default()
            })
            .unwrap();
        assert!(!h.copy_text("copied while disabled"));
        h.service
            .apply_settings(ClipboardSettings::default())
            .unwrap();
        assert!(!h.tick());
        assert_eq!(h.previews(""), ["after resume", "first"]);
        // Read failures are retried for markerless sources too.
        h.clipboard.fail_next_reads(1);
        h.clipboard.copy(ClipContent::Text("busy once".into()));
        assert!(!h.tick());
        assert!(h.tick());
    }

    #[test]
    fn markerless_empty_baseline_captures_first_copy_and_recopy_after_clear() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("clipboard");
        let mut h = Harness::with_clipboard(
            dir,
            path,
            ClipboardSettings::default(),
            MemoryClipboard::without_markers(),
        );
        assert!(
            !h.tick(),
            "an observed empty clipboard finishes the startup baseline"
        );
        assert!(h.copy_text("first copy"));
        assert_eq!(h.service.list("", None, 10).unwrap().total, 1);
        h.clipboard.clear();
        assert!(!h.tick());
        assert!(
            h.copy_text("first copy"),
            "a clear separates two actual copies"
        );
        assert_eq!(h.service.list("", None, 10).unwrap().items[0].copy_count, 2);
    }

    #[test]
    fn markerless_empty_baseline_after_pause_and_disable_does_not_eat_new_copies() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("clipboard");
        let clipboard = MemoryClipboard::without_markers();
        clipboard.copy(ClipContent::Text("present before startup".into()));
        let mut h = Harness::with_clipboard(dir, path, ClipboardSettings::default(), clipboard);
        assert!(!h.tick());
        assert!(h.copy_text("already saved"));
        h.service.set_paused(true).unwrap();
        h.clipboard.clear();
        assert!(!h.tick());
        h.service.set_paused(false).unwrap();
        assert!(!h.tick());
        assert!(h.copy_text("first after empty resume"));
        h.service
            .apply_settings(ClipboardSettings {
                enabled: false,
                ..Default::default()
            })
            .unwrap();
        h.clipboard.clear();
        assert!(!h.tick());
        h.service
            .apply_settings(ClipboardSettings::default())
            .unwrap();
        assert!(!h.tick());
        assert!(h.copy_text("first after empty enable"));
        assert_eq!(
            h.previews(""),
            [
                "first after empty enable",
                "first after empty resume",
                "already saved",
            ]
        );
    }

    #[test]
    fn markerless_failed_read_does_not_finish_the_startup_baseline() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("clipboard");
        let clipboard = MemoryClipboard::without_markers();
        clipboard.copy(ClipContent::Text("copied before startup".into()));
        clipboard.fail_next_reads(1);
        let mut h = Harness::with_clipboard(dir, path, ClipboardSettings::default(), clipboard);
        assert!(!h.tick(), "a busy clipboard has not yielded a baseline");
        assert!(
            !h.tick(),
            "the pre-existing payload is still only a baseline"
        );
        assert_eq!(h.service.list("", None, 10).unwrap().total, 0);
        assert!(h.copy_text("fresh after a confirmed baseline"));
    }

    #[test]
    fn missing_format_is_the_only_read_error_that_can_establish_an_empty_baseline() {
        let wrapped = |error: arboard::Error| {
            tauri_plugin_clipboard_manager::Error::Clipboard(error.to_string())
        };
        assert!(missing_clipboard_format(&wrapped(
            arboard::Error::ContentNotAvailable
        )));
        for error in [
            arboard::Error::ClipboardOccupied,
            arboard::Error::ConversionFailure,
            arboard::Error::ClipboardNotSupported,
            arboard::Error::Unknown {
                description: arboard::Error::ContentNotAvailable.to_string(),
            },
        ] {
            assert!(!missing_clipboard_format(&wrapped(error)));
        }
    }

    #[test]
    fn utility_copies_use_the_injected_writer_without_creating_history() {
        let mut harness = Harness::new(ClipboardSettings::default());
        harness
            .service
            .write_content(ClipContent::Text("0.333333333333".into()))
            .unwrap();
        assert_eq!(
            harness.clipboard.content(),
            Some(ClipContent::Text("0.333333333333".into()))
        );
        assert!(!harness.tick());
        assert_eq!(harness.service.status().unwrap().count, 0);
    }

    #[test]
    fn captures_text_links_and_images_and_classifies_them() {
        let mut h = Harness::new(ClipboardSettings::default());
        assert!(h.copy_text("회의록 초안"));
        assert!(h.copy_text("https://example.com/a?b=1"));
        assert!(h.copy_text("mailto:someone@example.com"));
        assert!(h.copy_text("not a link: https://example.com"));
        assert!(h.copy(image(300, 150, 7)));
        assert!(!h.copy_text("   \n\t"));
        let list = h.service.list("", None, 10).unwrap();
        let kinds: Vec<_> = list.items.iter().map(|item| item.kind).collect();
        assert_eq!(
            kinds,
            [
                ClipKind::Image,
                ClipKind::Text,
                ClipKind::Link,
                ClipKind::Link,
                ClipKind::Text
            ]
        );
        assert_eq!(list.items[0].preview, "300×150");
        assert_eq!(
            (list.items[0].width, list.items[0].height),
            (Some(300), Some(150))
        );
        assert_eq!(
            h.service.list("", Some(ClipKind::Link), 10).unwrap().total,
            2
        );
        let thumb = h.service.thumbnail_data_url(&list.items[0].id).unwrap();
        assert!(thumb.starts_with("data:image/png;base64,iVBORw0KGgo"));
    }

    #[test]
    fn duplicates_move_to_top_without_new_entries_and_restore_is_not_counted() {
        let mut h = Harness::new(ClipboardSettings::default());
        h.copy_text("first");
        h.copy_text("second");
        h.copy_text("first");
        let items = h.service.list("", None, 10).unwrap().items;
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].preview, "first");
        assert_eq!(items[0].copy_count, 2);
        let second = items[1].id.clone();
        h.service.copy_to_clipboard(&second).unwrap();
        assert!(
            !h.tick(),
            "the app's own write must not be recorded as a new copy"
        );
        let items = h.service.list("", None, 10).unwrap().items;
        assert_eq!(items.len(), 2);
        assert_eq!(
            (items[0].preview.as_str(), items[0].copy_count),
            ("second", 1)
        );
        // Restoring an image round-trips through the stored PNG exactly.
        h.copy(image(40, 20, 3));
        let id = h.service.list("", Some(ClipKind::Image), 1).unwrap().items[0]
            .id
            .clone();
        h.clipboard.copy(ClipContent::Text("other".into()));
        h.service.copy_to_clipboard(&id).unwrap();
        assert_eq!(h.clipboard.content(), Some(image(40, 20, 3)));
    }

    #[test]
    fn paused_disabled_and_excluded_content_is_never_recorded() {
        let mut h = Harness::new(ClipboardSettings::default());
        h.service.set_paused(true).unwrap();
        assert!(!h.copy_text("secret while paused"));
        h.service.set_paused(false).unwrap();
        assert!(
            !h.tick(),
            "content copied during pause stays unrecorded after resume"
        );
        h.clipboard.set_excluded(true);
        assert!(!h.copy_text("password-manager value"));
        h.clipboard.set_excluded(false);
        h.service
            .apply_settings(ClipboardSettings {
                enabled: false,
                ..Default::default()
            })
            .unwrap();
        assert!(!h.copy_text("disabled"));
        h.service
            .apply_settings(ClipboardSettings {
                capture_images: false,
                ..Default::default()
            })
            .unwrap();
        assert!(!h.copy(image(4, 4, 1)));
        assert!(h.copy_text("kept"));
        assert_eq!(h.previews(""), ["kept"]);
        assert!(!h.service.status().unwrap().paused);
    }

    #[test]
    fn transient_read_failures_retry_then_report_without_contents() {
        let mut h = Harness::new(ClipboardSettings::default());
        h.clipboard.fail_next_reads(2);
        h.clipboard.copy(ClipContent::Text("retry me".into()));
        assert!(!h.tick());
        assert!(!h.tick());
        assert!(h.tick());
        h.clipboard.fail_next_reads(99);
        h.clipboard.copy(ClipContent::Text("never readable".into()));
        let revision = h.service.status().unwrap().revision;
        for _ in 0..4 {
            assert!(!h.tick());
        }
        assert!(
            h.tick(),
            "diagnostic change must be emitted to visible windows"
        );
        assert!(h.service.status().unwrap().revision > revision);
        let error = h.service.status().unwrap().last_error.unwrap();
        assert!(!error.contains("never readable"));
    }

    #[test]
    fn payload_storage_failures_change_status_without_exposing_content() {
        let mut h = Harness::new(ClipboardSettings::default());
        fs::remove_dir(h.path.join("blobs")).unwrap();
        fs::write(h.path.join("blobs"), b"blocked fixture directory").unwrap();
        let revision = h.service.status().unwrap().revision;
        assert!(
            h.copy_text("private fixture value"),
            "storage diagnostic must reach the monitor listener"
        );
        let status = h.service.status().unwrap();
        assert_eq!(status.count, 0);
        assert!(status.revision > revision);
        assert!(!status.last_error.unwrap().contains("private fixture value"));
    }

    #[test]
    fn retention_prunes_by_age_count_and_size_but_keeps_pinned() {
        let mut h = Harness::new(ClipboardSettings {
            max_items: 10,
            ..Default::default()
        });
        for index in 0..12 {
            h.copy_text(&format!("item {index}"));
        }
        assert_eq!(h.service.list("", None, 100).unwrap().total, 10);
        assert!(!h.previews("").contains(&"item 0".to_string()));
        let oldest = h.service.list("item 2", None, 1).unwrap().items[0]
            .id
            .clone();
        h.service.set_pinned(&oldest, true).unwrap();
        for index in 12..30 {
            h.copy_text(&format!("item {index}"));
        }
        let previews = h.previews("");
        assert_eq!(previews.len(), 11);
        assert_eq!(
            previews[0], "item 2",
            "pinned items are listed first and kept"
        );
        // Age: backdate everything, then prune by applying settings.
        {
            let mut store = h.service.store.lock().unwrap();
            for stored in &mut store.items {
                stored.item.last_copied_ms -= 40 * 86_400_000;
            }
        }
        h.service
            .apply_settings(ClipboardSettings::default())
            .unwrap();
        assert_eq!(h.previews(""), ["item 2"]);
        // Size: a tiny byte budget drops unpinned items and their blobs.
        h.copy(image(64, 64, 9));
        {
            let mut store = h.service.store.lock().unwrap();
            store.settings.max_bytes = 1;
            store.prune(now_ms());
        }
        assert_eq!(h.previews(""), ["item 2"]);
        let blobs = fs::read_dir(h.path.join("blobs")).unwrap().count();
        assert_eq!(blobs, 1);
    }

    #[test]
    fn search_supports_and_terms_case_unicode_and_korean_initials() {
        let mut h = Harness::new(ClipboardSettings::default());
        h.copy_text("2026 공문서 계획");
        h.copy_text("Quarterly REPORT draft");
        h.copy_text("https://example.com/Report");
        h.copy_text("Cafe\u{301} menu");
        h.copy(image(2, 2, 0));
        assert_eq!(
            h.previews("report"),
            ["https://example.com/Report", "Quarterly REPORT draft"]
        );
        assert_eq!(h.previews("report draft"), ["Quarterly REPORT draft"]);
        assert_eq!(h.previews("ㄱㅁㅅ"), ["2026 공문서 계획"]);
        assert_eq!(h.previews("café"), ["Cafe\u{301} menu"]);
        assert_eq!(
            h.service.list("", Some(ClipKind::Image), 10).unwrap().total,
            1
        );
        assert!(h.service.list(&"x".repeat(5000), None, 10).is_err());
    }

    #[test]
    fn history_survives_restart_and_delete_clear_remove_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("clipboard");
        let mut h = Harness::with_dir(dir, path.clone(), ClipboardSettings::default());
        h.copy_text("persist me");
        h.copy(image(10, 10, 2));
        h.copy_text("pinned");
        let pinned = h.service.list("pinned", None, 1).unwrap().items[0]
            .id
            .clone();
        h.service.set_pinned(&pinned, true).unwrap();
        let before = h.service.list("", None, 10).unwrap().items;
        assert_eq!(
            *h.events.lock().unwrap(),
            1,
            "pinning notifies the listener"
        );
        let reopened = ClipboardService::open(path.clone(), ClipboardSettings::default(), |_| {});
        assert_eq!(reopened.list("", None, 10).unwrap().items, before);
        let filtered = reopened.list("pinned", None, 1).unwrap();
        assert_eq!(filtered.items.len(), 1);
        assert_eq!(filtered.status.count, 3);
        assert_eq!(filtered.status.unpinned_count, 2);
        let text = before
            .iter()
            .find(|item| item.preview == "persist me")
            .unwrap();
        assert_eq!(reopened.text(&text.id).unwrap(), "persist me");
        assert!(
            reopened.text(&before[1].id).is_err(),
            "image items have no text"
        );
        reopened.delete(&text.id).unwrap();
        assert!(reopened.text(&text.id).is_err());
        reopened.clear(false).unwrap();
        assert_eq!(reopened.list("", None, 10).unwrap().total, 1);
        assert_eq!(reopened.status().unwrap().unpinned_count, 0);
        reopened.clear(true).unwrap();
        assert_eq!(reopened.status().unwrap().count, 0);
        assert_eq!(fs::read_dir(path.join("blobs")).unwrap().count(), 0);
        assert!(reopened.delete("../settings").is_err());
    }

    #[test]
    fn corrupt_index_is_preserved_and_reported() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("clipboard");
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("index.json"), "{broken").unwrap();
        let service = ClipboardService::open(path.clone(), ClipboardSettings::default(), |_| {});
        let status = service.status().unwrap();
        assert_eq!(status.count, 0);
        assert!(status.last_error.unwrap().contains("index.corrupt-"));
        let backups: Vec<_> = fs::read_dir(&path)
            .unwrap()
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("index.corrupt-")
            })
            .collect();
        assert_eq!(fs::read_to_string(backups[0].path()).unwrap(), "{broken");
    }

    #[test]
    fn rejects_oversized_inputs_and_invalid_settings() {
        let mut h = Harness::new(ClipboardSettings::default());
        assert!(!h.copy_text(&"a".repeat(MAX_TEXT_BYTES + 1)));
        assert!(!h.copy(ClipContent::Image {
            width: 3,
            height: 3,
            rgba: vec![0; 5]
        }));
        assert!(ClipboardSettings::default().validate().is_ok());
        for invalid in [
            ClipboardSettings {
                max_items: 5,
                ..Default::default()
            },
            ClipboardSettings {
                max_age_days: 4000,
                ..Default::default()
            },
            ClipboardSettings {
                max_bytes: 1024,
                ..Default::default()
            },
        ] {
            assert!(invalid.validate().is_err());
        }
        let parsed: ClipboardSettings = serde_json::from_str(r#"{"maxItems":50}"#).unwrap();
        assert_eq!(
            (parsed.max_items, parsed.enabled, parsed.onboarded),
            (50, true, false)
        );
    }

    #[test]
    fn background_monitor_and_synthetic_capture_use_only_the_fixture() {
        let dir = tempfile::tempdir().unwrap();
        let events = Arc::new(Mutex::new(Vec::<ClipStatus>::new()));
        let sink = events.clone();
        let service = ClipboardService::open(
            dir.path().join("clipboard"),
            ClipboardSettings::default(),
            move |status| sink.lock().unwrap().push(status),
        );
        let clipboard = MemoryClipboard::default();
        clipboard.copy(ClipContent::Text("before start".into()));
        service.start(clipboard.boxed(), clipboard.boxed());
        std::thread::sleep(POLL_INTERVAL * 2);
        clipboard.copy(ClipContent::Text("https://example.com".into()));
        let start = std::time::Instant::now();
        while service.status().unwrap().count == 0 && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
        }
        let items = service.list("", None, 10).unwrap().items;
        assert_eq!(
            items.len(),
            1,
            "content present before start is not recorded"
        );
        assert_eq!(items[0].kind, ClipKind::Link);
        assert!(service.capture(image(8, 8, 5)).unwrap());
        assert!(!service.capture(ClipContent::Text(" ".into())).unwrap());
        assert_eq!(service.status().unwrap().count, 2);
        let last = events.lock().unwrap().last().cloned().unwrap();
        assert_eq!(last.count, 2);
        service.copy_to_clipboard(&items[0].id).unwrap();
        assert_eq!(
            clipboard.content(),
            Some(ClipContent::Text("https://example.com".into()))
        );
    }

    #[test]
    fn helpers_encode_and_downscale_correctly() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        let (w, h, data) = thumbnail(1000, 500, &vec![200; 1000 * 500 * 4], 256);
        assert_eq!((w, h, data.len()), (256, 128, 256 * 128 * 4));
        assert!(data.iter().all(|&value| value == 200));
        assert_eq!(
            format!("{:?}", ClipContent::Text("secret".into())),
            "Text(6 bytes)"
        );
    }
}

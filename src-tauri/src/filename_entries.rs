//! The live filename catalog storage.
//!
//! Entries are kept in the compact tree catalog (`filename_compact.rs`): every
//! directory once, every entry as a small record, about a fifth of the memory
//! of one owned `Entry` per file. `Entry` stays the owned, short-lived form
//! that preparation workers build (Unicode normalization and Hangul
//! projections are the expensive part); inserting copies it into compact
//! storage and drops it.
//!
//! Searches never rebuild an owned `Entry`. A `View` borrows the whole-path
//! search text and projections from a per-thread `Scratch` buffer instead.
use super::compact::{self, EntryRef, SearchScratch};
use super::*;

/// Byte offset of the file name in a search text. Entries without a name
/// (a drive root such as `C:\`) rank by an empty name, like before.
fn name_start(search: &str, has_name: bool) -> u32 {
    if has_name {
        search.rfind('/').map_or(0, |index| index + 1) as u32
    } else {
        search.len() as u32
    }
}

impl Entry {
    /// The search-text file name (tests read owned copies of stored entries).
    #[cfg(test)]
    pub(super) fn name(&self) -> &str {
        &self.searchable()[self.name_start as usize..]
    }
    /// Borrowed compact-catalog input for this owned entry.
    fn input(&self) -> compact::EntryInput<'_> {
        let search = self.searchable();
        compact::EntryInput {
            path: &self.path,
            key: &self.key,
            search: self.search.as_deref(),
            initials: self.initials_path.as_deref(),
            keyboard: self.keyboard_path.as_deref(),
            // Keys never end in '/', so a named entry's name starts before the
            // end of a non-empty search text; a nameless one starts at its end.
            has_name: (self.name_start as usize) < search.len(),
            is_dir: self.is_dir,
            size: self.size,
            modified_ms: self.modified_ms,
            seen: self.seen,
        }
    }
    /// The matching view of an owned entry, for tests.
    #[cfg(test)]
    pub(super) fn view(&self) -> View<'_> {
        View {
            search: self.searchable(),
            initials_path: self.initials_path.as_deref(),
            keyboard_path: self.keyboard_path.as_deref(),
            name_start: self.name_start as usize,
            is_dir: self.is_dir,
            modified_ms: self.modified_ms,
        }
    }
}

/// What a filename query reads from one entry. Field names follow `Entry`.
pub(super) struct View<'a> {
    search: &'a str,
    pub(super) initials_path: Option<&'a str>,
    pub(super) keyboard_path: Option<&'a str>,
    name_start: usize,
    pub(super) is_dir: bool,
    pub(super) modified_ms: Option<i64>,
}
impl<'a> View<'a> {
    pub(super) fn searchable(&self) -> &'a str {
        self.search
    }
    pub(super) fn name(&self) -> &'a str {
        &self.search[self.name_start..]
    }
    pub(super) fn initials_name(&self) -> Option<&'a str> {
        self.initials_path
            .and_then(|initials| initials.get(self.name_start..))
    }
    pub(super) fn extension(&self) -> &'a str {
        let name = self.name();
        match name.rfind('.') {
            Some(index) if index > 0 => &name[index + 1..],
            _ => "",
        }
    }
}

/// Per-thread buffers that whole-path texts are rebuilt into during a scan.
#[derive(Default)]
pub(super) struct Scratch {
    search: SearchScratch,
    initials: String,
    keyboard: String,
}

/// Rebuild what a query reads from a stored entry. The keyboard projection is
/// only needed by the keyboard-layout correction pass.
pub(super) fn view<'s>(entry: EntryRef<'_>, scratch: &'s mut Scratch, keyboard: bool) -> View<'s> {
    let search = entry.search_text(&mut scratch.search);
    let initials_path = entry.write_initials(&mut scratch.initials);
    let keyboard_path = if keyboard {
        entry.write_keyboard(&mut scratch.keyboard)
    } else {
        None
    };
    View {
        name_start: name_start(search, entry.has_name()) as usize,
        search,
        initials_path,
        keyboard_path,
        is_dir: entry.is_dir(),
        modified_ms: entry.modified_ms(),
    }
}

/// The search result for a stored entry (same fields as `Entry::result`).
pub(super) fn result(entry: EntryRef<'_>) -> FileResult {
    let mut buffer = String::new();
    let path = entry.write_path(&mut buffer).to_string();
    let name = path
        .rsplit(['\\', '/'])
        .find(|part| !part.is_empty())
        .unwrap_or(&path)
        .to_string();
    FileResult {
        path,
        name,
        is_dir: entry.is_dir(),
        size: entry.size(),
        modified_ms: entry.modified_ms(),
    }
}

/// Whether a walked row equals the stored entry, so only its sweep id needs
/// refreshing (same rule as the original `Entry::unchanged`).
pub(super) fn unchanged(entry: EntryRef<'_>, raw: &Raw, buffer: &mut String) -> bool {
    entry.is_dir() == raw.is_dir
        && entry.size() == raw.size
        && entry.modified_ms() == raw.modified_ms
        && entry.write_path(buffer) == raw.path
}

/// An owned copy of a stored entry, field for field as `Entry::new` built it.
#[cfg_attr(not(any(windows, test)), allow(dead_code))] // NTFS service catalog only.
pub(super) fn owned(entry: EntryRef<'_>) -> Entry {
    let mut buffer = String::new();
    let path: Box<str> = entry.write_path(&mut buffer).into();
    let key: Arc<str> = entry.write_key(&mut buffer).into();
    let search: Option<Box<str>> = entry
        .has_search()
        .then(|| entry.write_search(&mut buffer).into());
    let initials_path = entry.write_initials(&mut buffer).map(Into::into);
    let keyboard_path = entry.write_keyboard(&mut buffer).map(Into::into);
    let name_start = name_start(search.as_deref().unwrap_or(&key), entry.has_name());
    Entry {
        path,
        key,
        search,
        name_start,
        initials_path,
        keyboard_path,
        is_dir: entry.is_dir(),
        size: entry.size(),
        modified_ms: entry.modified_ms(),
        seen: u64::from(entry.seen()),
    }
}

/// The catalog's entry storage.
#[derive(Default)]
pub(super) struct Entries {
    inner: compact::Catalog,
}

impl Entries {
    #[cfg_attr(not(any(windows, test)), allow(dead_code))] // NTFS service catalog only.
    pub(super) fn new() -> Self {
        Self::default()
    }
    /// Entries ever stored; unchanged entries that are only refreshed do not count.
    #[cfg(test)]
    pub(super) fn insertions(&self) -> u64 {
        self.inner.insertions()
    }
    pub(super) fn len(&self) -> usize {
        self.inner.len()
    }
    pub(super) fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
    pub(super) fn clear(&mut self) {
        self.inner.clear();
    }
    /// Insert or replace the entry with the same key. Returns its id.
    pub(super) fn insert(&mut self, entry: &Entry) -> u32 {
        self.inner.insert(&entry.input())
    }
    pub(super) fn get(&self, key: &str) -> Option<EntryRef<'_>> {
        self.inner.get(key)
    }
    pub(super) fn contains_key(&self, key: &str) -> bool {
        self.inner.contains_key(key)
    }
    pub(super) fn remove(&mut self, key: &str) -> bool {
        self.inner.remove(key)
    }
    /// Refresh the sweep id of an unchanged entry.
    pub(super) fn set_seen(&mut self, id: u32, sweep: u64) {
        self.inner.set_seen(id, sweep);
    }
    /// Keep only entries for which `keep(key, entry)` returns true.
    pub(super) fn retain(&mut self, mut keep: impl FnMut(&str, EntryRef<'_>) -> bool) {
        let mut key = String::new();
        self.inner.retain(|entry| {
            let entry_key = entry.write_key(&mut key);
            keep(entry_key, entry)
        });
    }
    /// The entry at `root` and every entry below it.
    pub(super) fn subtree<'a>(&'a self, root: &'a str) -> impl Iterator<Item = EntryRef<'a>> + 'a {
        self.inner.subtree(root)
    }
    /// All live entries in storage order.
    pub(super) fn refs(&self) -> impl Iterator<Item = EntryRef<'_>> + '_ {
        self.inner.iter()
    }
    pub(super) fn entry(&self, id: u32) -> Option<EntryRef<'_>> {
        self.inner.entry(id)
    }
    pub(super) fn slots(&self) -> usize {
        self.inner.slots()
    }
    /// Release spare capacity after a bulk build (vectors grow by doubling,
    /// so a just-built catalog can hold up to twice the memory it needs).
    #[cfg_attr(not(any(windows, test)), allow(dead_code))] // NTFS service catalog only.
    pub(super) fn shrink_to_fit(&mut self) {
        self.inner.shrink_to_fit();
    }
    /// Add every entry of `other`; on equal keys `other` wins, like the
    /// `BTreeMap::append` merge of private worker trees did.
    #[cfg(test)]
    pub(super) fn merge(&mut self, other: Entries) {
        for entry in other.refs() {
            self.insert(&owned(entry));
        }
    }
    /// Owned entries, consuming the storage (bounded batches are taken by
    /// the caller; each owned entry exists only while it is being published).
    #[cfg_attr(not(any(windows, test)), allow(dead_code))] // NTFS service catalog only.
    pub(super) fn into_entries(self) -> impl Iterator<Item = Entry> {
        let mut id = 0u32;
        std::iter::from_fn(move || {
            while (id as usize) < self.slots() {
                let current = id;
                id += 1;
                if let Some(entry) = self.entry(current) {
                    return Some(owned(entry));
                }
            }
            None
        })
    }
    /// Owned entries sorted by key, for tests that compare whole catalogs.
    #[cfg(test)]
    pub(super) fn sorted(&self) -> Vec<Entry> {
        let mut entries: Vec<Entry> = self.refs().map(owned).collect();
        entries.sort_by(|left, right| left.key.cmp(&right.key));
        entries
    }
    /// Owned entries in key order (what iterating the old map yielded).
    #[cfg(test)]
    pub(super) fn values(&self) -> std::vec::IntoIter<Entry> {
        self.sorted().into_iter()
    }
    /// Keys in key order.
    #[cfg(test)]
    pub(super) fn keys(&self) -> std::vec::IntoIter<Arc<str>> {
        self.sorted()
            .into_iter()
            .map(|entry| entry.key)
            .collect::<Vec<_>>()
            .into_iter()
    }
}

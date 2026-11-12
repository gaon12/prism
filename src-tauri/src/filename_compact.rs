//! Compact in-memory filename catalog (tree layout: shared directories + one text arena).
//!
//! The original catalog kept two full path strings per entry (display path and
//! identity key) inside a `BTreeMap`: about 600 bytes per entry, 1.3 GB for a
//! 2.2-million-entry drive, far beyond a 512 MB low-end PC.
//!
//! Almost all of those bytes repeat the parent directory. This catalog stores:
//! - every **directory once** as a 16-byte record whose path lives in a shared
//!   text arena (its identity key is derived from the path when possible);
//! - every **entry as a 40-byte record** (`Node`) that points to its parent
//!   directory and to its own name in the same arena;
//! - two small open-addressing tables of `u32` ids for key lookups, instead of
//!   hash maps that duplicate keys.
//!
//! Every string the rest of the index used to read (display path, key, search
//! text, Hangul projections) is rebuilt **exactly**, byte for byte, by joining
//! the directory part with the entry's own part. Tests check that round trip,
//! so search semantics stay identical while memory drops to about a fifth
//! (958 MiB to 186 MiB on a real 2-million-entry drive).
//!
//! This module has no dependencies: callers pass already-normalized strings.

use std::hash::{BuildHasher, RandomState};

/// "No value" in 32-bit link and offset fields.
const NONE: u32 = u32::MAX;
/// A deleted slot in an open-addressing table (keeps probe chains intact).
const TOMB: u32 = u32::MAX - 1;

// Node and directory flag bits.
const IS_DIR: u8 = 1;
/// The display path joins its directory with `\` (Windows) instead of `/`.
const BACKSLASH: u8 = 2;
/// The key part is the ASCII-lowercased display part (with `/` separators for
/// directories) and nothing else differs. Otherwise the parts are in `extra`.
const DERIVED: u8 = 4;
/// The entry sits under the sentinel root directory: its strings are not joined
/// with a separator (for example the drive root `c:` itself).
const TOP: u8 = 8;
/// `Node::modified` holds a time. A separate bit (not a sentinel value) keeps
/// every `Some(i64)` distinct from `None`, including `Some(i64::MIN)`.
const HAS_TIME: u8 = 16;
/// The entry has no file name for ranking (`EntryInput::has_name` false).
const NAMELESS: u8 = 32;
/// Parts of a stored block: key, search, initials, keyboard and (entries only)
/// the entry's own display directory when it differs from the shared record.
const PARTS: usize = 5;
/// Block part index of an entry's own display directory.
const OWN_DIR: usize = 4;

/// Everything the index needs to (re)build one entry. All strings are whole
/// paths, exactly as the original `Entry` stored or derived them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntryInput<'a> {
    /// Display path in the platform's own form, for example `C:\Docs\a.txt`.
    pub path: &'a str,
    /// Identity key: lowercase on Windows, `/` separators, no trailing `/`.
    pub key: &'a str,
    /// Search text when it differs from `key` (NFC/lowercase).
    pub search: Option<&'a str>,
    /// Hangul initials projection of the search text.
    pub initials: Option<&'a str>,
    /// Keyboard (Korean 2-set) projection of the search text.
    pub keyboard: Option<&'a str>,
    /// False for an entry that ranks by an empty name (a walked drive root
    /// keeps its name; the service's `C:\` row does not). Stored as given.
    pub has_name: bool,
    pub is_dir: bool,
    pub size: u64,
    pub modified_ms: Option<i64>,
    /// Sweep id. Stored modulo 2^32: sweeps are only compared for equality.
    pub seen: u64,
}

/// One directory, shared by all of its children. 16 bytes.
#[derive(Clone, Copy, Default)]
struct Dir {
    /// Display path in `text`.
    path: u32,
    path_len: u32,
    /// Stored key/search/initials/keyboard block in `text`, or `NONE` when the
    /// key is derived from the path and there is nothing else.
    extra: u32,
    /// Live child entries; the record is released at zero. 0 = free slot.
    children: u32,
}

/// One entry. 40 bytes; its name bytes live in `Catalog::text`.
#[derive(Clone, Copy)]
struct Node {
    /// Parent directory id, or `NONE` for a free slot.
    dir: u32,
    /// Display name in `text`.
    name: u32,
    /// Stored key/search/initials/keyboard name block, or `NONE` when derived.
    extra: u32,
    /// Free-list link for free slots.
    next: u32,
    name_len: u16,
    flags: u8,
    seen: u32,
    size: u64,
    modified: i64,
}

// ----- stored blocks -----
//
// A block holds `PARTS` optional strings, each written as a u32 little-endian
// length (`NONE` when absent) followed by the bytes. The functions below read
// a block from a slice that starts at the block (see `Text::from`).

/// Bytes a block of these parts takes.
fn block_size(parts: &[Option<&str>; PARTS]) -> usize {
    parts.iter().map(|part| 4 + part.map_or(0, str::len)).sum()
}
fn write_block(out: &mut Vec<u8>, parts: [Option<&str>; PARTS]) {
    for part in parts {
        match part {
            Some(value) => {
                out.extend_from_slice(&(value.len() as u32).to_le_bytes());
                out.extend_from_slice(value.as_bytes());
            }
            None => out.extend_from_slice(&NONE.to_le_bytes()),
        }
    }
}
/// Part `index` of the block at the start of `block`.
fn get_block(block: &[u8], index: usize) -> Option<&str> {
    let mut at = 0;
    for current in 0..=index {
        let len = u32::from_le_bytes(block[at..at + 4].try_into().expect("4 bytes"));
        at += 4;
        if len == NONE {
            if current == index {
                return None;
            }
            continue;
        }
        let end = at + len as usize;
        if current == index {
            // Only valid UTF-8 slices of caller strings are ever stored.
            return Some(std::str::from_utf8(&block[at..end]).expect("stored UTF-8"));
        }
        at = end;
    }
    None
}
/// Length of the block at the start of `block`.
fn block_len(block: &[u8]) -> usize {
    let mut at = 0;
    for _ in 0..PARTS {
        let len = u32::from_le_bytes(block[at..at + 4].try_into().expect("4 bytes"));
        at += 4;
        if len != NONE {
            at += len as usize;
        }
    }
    at
}

// ----- chunked storage -----
//
// Why chunks: a single growing `Vec` reallocates by doubling, and while it
// copies, the old and the new buffer exist together. On a 2-million-entry
// drive the entry records and the text each passed through such a copy, and
// the build briefly needed more than twice the finished catalog (423 MB
// peak for a 190 MB result), which alone broke a 512 MB machine. Chunks never
// move, so the peak stays close to the finished size.

/// A text chunk holds 4 MiB.
const TEXT_CHUNK_BITS: u32 = 22;
const TEXT_CHUNK: usize = 1 << TEXT_CHUNK_BITS;
const TEXT_POSITION: u32 = (1 << TEXT_CHUNK_BITS) - 1;

/// Append-only bytes for names, directory paths and blocks, in 4 MiB chunks.
///
/// Every item is placed inside one chunk, so a reader always gets one
/// contiguous slice, and an offset stays a `u32`: the chunk number in the high
/// bits and the position inside it in the low 22 bits. An item larger than a
/// chunk (impossible for real paths, but handled) gets a chunk of its own at
/// position 0. The first chunk starts small and doubles up to 4 MiB, so a
/// small catalog stays small.
#[derive(Default)]
struct Text {
    chunks: Vec<Vec<u8>>,
    /// Bytes written, including bytes that later became garbage. Unused chunk
    /// tails are not counted.
    used: usize,
}
impl Text {
    /// Make room for one `len`-byte item. Returns its offset and the chunk to
    /// append it to.
    fn place(&mut self, len: usize) -> (u32, &mut Vec<u8>) {
        // Append to the last chunk only when the whole item fits and starts
        // strictly inside the chunk (a full chunk has no position left).
        let fits = self
            .chunks
            .last()
            .is_some_and(|chunk| chunk.len() < TEXT_CHUNK && chunk.len() + len <= TEXT_CHUNK);
        if !fits {
            // Later chunks are only needed by big catalogs: allocate them whole.
            let capacity = if len > TEXT_CHUNK {
                len
            } else if self.chunks.is_empty() {
                0
            } else {
                TEXT_CHUNK
            };
            self.chunks.push(Vec::with_capacity(capacity));
        }
        let number = u32::try_from(self.chunks.len() - 1)
            .ok()
            .filter(|number| *number < 1 << (32 - TEXT_CHUNK_BITS))
            .expect("catalog text arena below 4 GiB");
        let chunk = self.chunks.last_mut().expect("a chunk was just ensured");
        if chunk.capacity() - chunk.len() < len {
            // Only the first, still small chunk grows; it stops at 4 MiB.
            let target = (chunk.capacity() * 2)
                .max(chunk.len() + len)
                .max(256)
                .min(TEXT_CHUNK.max(chunk.len() + len));
            chunk.reserve_exact(target - chunk.len());
        }
        let offset = number << TEXT_CHUNK_BITS | chunk.len() as u32;
        (offset, chunk)
    }
    /// Store bytes; returns their offset.
    fn push(&mut self, bytes: &[u8]) -> u32 {
        let (offset, chunk) = self.place(bytes.len());
        chunk.extend_from_slice(bytes);
        self.used += bytes.len();
        offset
    }
    /// Store a block of `PARTS` optional strings; returns its offset.
    fn put_block(&mut self, parts: [Option<&str>; PARTS]) -> u32 {
        let size = block_size(&parts);
        let (offset, chunk) = self.place(size);
        write_block(chunk, parts);
        self.used += size;
        offset
    }
    /// The chunk holding `offset` and the position of `offset` inside it.
    fn locate(&self, offset: u32) -> (&[u8], usize) {
        let chunk = &self.chunks[(offset >> TEXT_CHUNK_BITS) as usize];
        (chunk, (offset & TEXT_POSITION) as usize)
    }
    fn bytes(&self, offset: u32, len: usize) -> &[u8] {
        let (chunk, at) = self.locate(offset);
        &chunk[at..at + len]
    }
    /// Everything from `offset` to the end of its chunk (for block readers).
    fn from(&self, offset: u32) -> &[u8] {
        let (chunk, at) = self.locate(offset);
        &chunk[at..]
    }
    /// Bytes of the stored block at `offset`.
    fn block(&self, offset: u32) -> &[u8] {
        let block = self.from(offset);
        &block[..block_len(block)]
    }
    fn reserve(&mut self, bytes: usize) {
        self.chunks.reserve(bytes / TEXT_CHUNK + 1);
    }
    /// Release the spare room of the last chunk (earlier chunks are full).
    fn shrink_to_fit(&mut self) {
        if let Some(last) = self.chunks.last_mut() {
            last.shrink_to_fit();
        }
        self.chunks.shrink_to_fit();
    }
    fn capacity(&self) -> usize {
        self.chunks.iter().map(Vec::capacity).sum::<usize>()
            + self.chunks.capacity() * std::mem::size_of::<Vec<u8>>()
    }
}

/// Entry records per chunk: 65,536 records, 2.5 MiB.
const NODE_CHUNK_BITS: u32 = 16;
const NODE_CHUNK: usize = 1 << NODE_CHUNK_BITS;

/// Entry records in fixed chunks, indexed like a `Vec` (`nodes[id]`).
#[derive(Default)]
struct Nodes {
    chunks: Vec<Vec<Node>>,
    len: usize,
}
impl Nodes {
    fn len(&self) -> usize {
        self.len
    }
    fn push(&mut self, node: Node) {
        if self.len.is_multiple_of(NODE_CHUNK) {
            // A second chunk means a big catalog: allocate it whole.
            let capacity = if self.chunks.is_empty() {
                0
            } else {
                NODE_CHUNK
            };
            self.chunks.push(Vec::with_capacity(capacity));
        }
        let chunk = self.chunks.last_mut().expect("a chunk was just ensured");
        if chunk.len() == chunk.capacity() {
            // Only the first chunk grows; it stops at one full chunk.
            let target = (chunk.capacity() * 2).clamp(16, NODE_CHUNK);
            chunk.reserve_exact(target - chunk.len());
        }
        chunk.push(node);
        self.len += 1;
    }
    fn get(&self, index: usize) -> Option<&Node> {
        self.chunks
            .get(index >> NODE_CHUNK_BITS)?
            .get(index & (NODE_CHUNK - 1))
    }
    fn iter_mut(&mut self) -> impl Iterator<Item = &mut Node> {
        self.chunks.iter_mut().flatten()
    }
    fn reserve(&mut self, additional: usize) {
        self.chunks.reserve(additional / NODE_CHUNK + 1);
    }
    /// Release the spare room of the last chunk (earlier chunks are full).
    fn shrink_to_fit(&mut self) {
        if let Some(last) = self.chunks.last_mut() {
            last.shrink_to_fit();
        }
        self.chunks.shrink_to_fit();
    }
    fn capacity_bytes(&self) -> usize {
        self.chunks
            .iter()
            .map(|chunk| chunk.capacity() * std::mem::size_of::<Node>())
            .sum::<usize>()
            + self.chunks.capacity() * std::mem::size_of::<Vec<Node>>()
    }
}
impl std::ops::Index<usize> for Nodes {
    type Output = Node;
    fn index(&self, index: usize) -> &Node {
        &self.chunks[index >> NODE_CHUNK_BITS][index & (NODE_CHUNK - 1)]
    }
}
impl std::ops::IndexMut<usize> for Nodes {
    fn index_mut(&mut self, index: usize) -> &mut Node {
        &mut self.chunks[index >> NODE_CHUNK_BITS][index & (NODE_CHUNK - 1)]
    }
}

/// Lowercase ASCII and turn `\` into `/`: how Windows derives identity keys.
/// Only called for ASCII text (non-ASCII keys are always stored), so whole
/// segments are copied and lowercased in place instead of char by char.
fn derive_key(display: &str, buffer: &mut String) {
    let start = buffer.len();
    for (index, segment) in display.split('\\').enumerate() {
        if index > 0 {
            buffer.push('/');
        }
        buffer.push_str(segment);
    }
    buffer[start..].make_ascii_lowercase();
}

/// Open-addressing table of `u32` ids. The caller supplies hashes and equality,
/// so keys are never duplicated in memory. Linear probing, power-of-two size.
#[derive(Default)]
struct Table {
    slots: Vec<u32>,
    /// One hash byte per slot, to skip most key reconstructions while probing.
    tags: Vec<u8>,
    used: usize,
    tombs: usize,
}
impl Table {
    fn tag(hash: u64) -> u8 {
        (hash >> 57) as u8 | 0x80
    }
    fn find(&self, hash: u64, mut equal: impl FnMut(u32) -> bool) -> Option<usize> {
        if self.slots.is_empty() {
            return None;
        }
        let mask = self.slots.len() - 1;
        let tag = Self::tag(hash);
        let mut at = hash as usize & mask;
        loop {
            match self.slots[at] {
                NONE => return None,
                TOMB => {}
                id if self.tags[at] == tag && equal(id) => return Some(at),
                _ => {}
            }
            at = (at + 1) & mask;
        }
    }
    /// Insert an id known to be absent. `rehash` recomputes any stored id's hash.
    fn insert(&mut self, hash: u64, id: u32, rehash: impl Fn(u32) -> u64) {
        if (self.used + self.tombs + 1) * 4 > self.slots.len() * 3 {
            self.grow(rehash);
        }
        let mask = self.slots.len() - 1;
        let mut at = hash as usize & mask;
        while self.slots[at] != NONE && self.slots[at] != TOMB {
            at = (at + 1) & mask;
        }
        if self.slots[at] == TOMB {
            self.tombs -= 1;
        }
        self.slots[at] = id;
        self.tags[at] = Self::tag(hash);
        self.used += 1;
    }
    fn remove_at(&mut self, at: usize) {
        self.slots[at] = TOMB;
        self.used -= 1;
        self.tombs += 1;
    }
    fn grow(&mut self, rehash: impl Fn(u32) -> u64) {
        // Double only when live ids need it; otherwise just clear tombstones.
        self.rebuild(self.used + 1, rehash);
    }
    /// Make room for `ids` live ids without further growth (inserts grow
    /// above 3/4 load; this keeps load at most 1/2).
    // Only used by tools/catalog-qa through `Catalog::reserve`.
    #[allow(dead_code)]
    fn reserve(&mut self, ids: usize, rehash: impl Fn(u32) -> u64) {
        self.rebuild(ids, rehash);
    }
    /// Rebuild with at least twice `ids` slots (load at most one half).
    fn rebuild(&mut self, ids: usize, rehash: impl Fn(u32) -> u64) {
        let mut size = self.slots.len().max(16);
        while ids * 2 > size {
            size *= 2;
        }
        let old = std::mem::take(&mut self.slots);
        self.slots = vec![NONE; size];
        self.tags = vec![0; size];
        self.used = 0;
        self.tombs = 0;
        let mask = size - 1;
        for id in old.into_iter().filter(|&id| id != NONE && id != TOMB) {
            let hash = rehash(id);
            let mut at = hash as usize & mask;
            while self.slots[at] != NONE {
                at = (at + 1) & mask;
            }
            self.slots[at] = id;
            self.tags[at] = Self::tag(hash);
            self.used += 1;
        }
    }
    #[allow(dead_code)] // Only through `Catalog::heap_bytes` (tools/catalog-qa).
    fn bytes(&self) -> usize {
        self.slots.capacity() * 4 + self.tags.capacity()
    }
}

/// The compact catalog. Entry ids are stable until the entry is removed.
pub struct Catalog {
    dirs: Vec<Dir>,
    free_dirs: Vec<u32>,
    dir_table: Table,
    nodes: Nodes,
    free_nodes: u32,
    /// Names, directory paths and stored blocks.
    text: Text,
    key_table: Table,
    hasher: RandomState,
    len: usize,
    /// Entries ever stored (new or replacement). Refreshing metadata or a
    /// sweep id does not count, so callers can tell reuse from rebuilds.
    insertions: u64,
    /// Bytes in `text` no longer referenced; compacted when large.
    garbage: usize,
}

impl Default for Catalog {
    fn default() -> Self {
        Self::new()
    }
}

/// A borrowed view of one entry. Whole-path strings are written into a
/// caller-provided buffer so hot search loops never allocate per entry.
#[derive(Clone, Copy)]
pub struct EntryRef<'a> {
    catalog: &'a Catalog,
    id: u32,
}

impl Catalog {
    pub fn new() -> Self {
        Self {
            // Directory 0 is the sentinel parent of top-level entries such as `c:`.
            // It is never released (its child count starts above zero).
            dirs: vec![Dir {
                children: 1,
                ..Dir::default()
            }],
            free_dirs: Vec::new(),
            dir_table: Table::default(),
            nodes: Nodes::default(),
            free_nodes: NONE,
            text: Text::default(),
            key_table: Table::default(),
            hasher: RandomState::new(),
            len: 0,
            insertions: 0,
            garbage: 0,
        }
    }
    #[allow(dead_code)] // Read by tests that check unchanged rows are not rebuilt.
    pub fn insertions(&self) -> u64 {
        self.insertions
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn clear(&mut self) {
        *self = Self::new();
    }
    // Also used by tools/catalog-qa (memory benchmark and diagnostics).
    #[allow(dead_code)]
    /// Pre-size storage before a bulk build of `entries` rows whose display
    /// paths total about `path_bytes`, avoiding doubling slack.
    pub fn reserve(&mut self, entries: usize, path_bytes: usize) {
        self.nodes.reserve(entries);
        // Names plus directory paths measured 35% of whole-path bytes on a
        // real 2-million-entry system drive. Storage grows in chunks, so this
        // only sizes the chunk lists.
        self.text.reserve(path_bytes / 5 * 2);
        // Size the key table once, instead of growing through every doubling.
        if self.key_table.slots.len() < (self.len + entries) * 2 {
            let mut table = std::mem::take(&mut self.key_table);
            table.reserve(self.len + entries, |stored| {
                let mut scratch = String::new();
                self.hash(
                    (EntryRef {
                        catalog: self,
                        id: stored,
                    })
                    .write_key(&mut scratch),
                )
            });
            self.key_table = table;
        }
    }
    /// Release spare capacity after a bulk build.
    pub fn shrink_to_fit(&mut self) {
        self.nodes.shrink_to_fit();
        self.dirs.shrink_to_fit();
        self.text.shrink_to_fit();
        self.free_dirs.shrink_to_fit();
    }

    fn hash(&self, key: &str) -> u64 {
        self.hasher.hash_one(key)
    }
    fn text_str(&self, offset: u32, len: usize) -> &str {
        std::str::from_utf8(self.text.bytes(offset, len)).expect("stored UTF-8")
    }

    // ----- directories -----

    fn dir_path(&self, id: u32) -> &str {
        let dir = &self.dirs[id as usize];
        self.text_str(dir.path, dir.path_len as usize)
    }
    /// Stored part `index` (0 key, 1 search, 2 initials, 3 keyboard) of a directory.
    fn dir_part(&self, id: u32, index: usize) -> Option<&str> {
        let dir = &self.dirs[id as usize];
        (dir.extra != NONE)
            .then(|| get_block(self.text.from(dir.extra), index))
            .flatten()
    }
    /// Write a directory's identity key.
    fn write_dir_key<'b>(&self, id: u32, buffer: &'b mut String) -> &'b str {
        buffer.clear();
        match self.dir_part(id, 0) {
            Some(key) => buffer.push_str(key),
            None => derive_key(self.dir_path(id), buffer),
        }
        buffer.as_str()
    }
    fn find_dir(&self, key: &str) -> Option<u32> {
        let mut scratch = String::new();
        self.dir_table
            .find(self.hash(key), |id| {
                self.write_dir_key(id, &mut scratch) == key
            })
            .map(|at| self.dir_table.slots[at])
    }
    fn intern_dir(&mut self, split: &Split<'_>) -> u32 {
        if split.top {
            return 0;
        }
        if let Some(id) = self.find_dir(split.dir_key) {
            self.dirs[id as usize].children += 1;
            // Projections are deterministic per directory, so a later child can
            // only add what the first child did not need. Store them now.
            let plain_search = self.dir_part(id, 1).unwrap_or(split.dir_key).to_string();
            let add_initials = self.dir_part(id, 2).is_none()
                && split.dir_initials.is_some_and(|text| text != plain_search);
            let add_keyboard = self.dir_part(id, 3).is_none()
                && split.dir_keyboard.is_some_and(|text| text != plain_search);
            if add_initials || add_keyboard {
                let parts = [
                    self.dir_part(id, 0).map(str::to_string),
                    self.dir_part(id, 1).map(str::to_string),
                    self.dir_part(id, 2)
                        .map(str::to_string)
                        .or_else(|| add_initials.then(|| split.dir_initials.unwrap().to_string())),
                    self.dir_part(id, 3)
                        .map(str::to_string)
                        .or_else(|| add_keyboard.then(|| split.dir_keyboard.unwrap().to_string())),
                ];
                let old = self.dirs[id as usize].extra;
                if old != NONE {
                    self.garbage += block_len(self.text.from(old));
                }
                // A stored block always carries the key, so derivation is off.
                let key = parts[0]
                    .clone()
                    .unwrap_or_else(|| split.dir_key.to_string());
                let block = self.text.put_block([
                    Some(&key),
                    parts[1].as_deref(),
                    parts[2].as_deref(),
                    parts[3].as_deref(),
                    None,
                ]);
                self.dirs[id as usize].extra = block;
            }
            return id;
        }
        let search = split.dir_search.filter(|text| *text != split.dir_key);
        let plain = search.unwrap_or(split.dir_key);
        let initials = split.dir_initials.filter(|text| *text != plain);
        let keyboard = split.dir_keyboard.filter(|text| *text != plain);
        let mut derived = String::with_capacity(split.dir_path.len());
        derive_key(split.dir_path, &mut derived);
        let key_derived = split.dir_path.is_ascii() && derived == split.dir_key;
        let path = self.text.push(split.dir_path.as_bytes());
        let extra = if key_derived && search.is_none() && initials.is_none() && keyboard.is_none() {
            NONE
        } else {
            self.text
                .put_block([Some(split.dir_key), search, initials, keyboard, None])
        };
        let dir = Dir {
            path,
            path_len: split.dir_path.len() as u32,
            extra,
            children: 1,
        };
        let id = match self.free_dirs.pop() {
            Some(id) => {
                self.dirs[id as usize] = dir;
                id
            }
            None => {
                self.dirs.push(dir);
                (self.dirs.len() - 1) as u32
            }
        };
        let hash = self.hash(split.dir_key);
        let mut table = std::mem::take(&mut self.dir_table);
        table.insert(hash, id, |stored| {
            let mut scratch = String::new();
            self.hash(self.write_dir_key(stored, &mut scratch))
        });
        self.dir_table = table;
        id
    }
    fn release_dir(&mut self, id: u32) {
        self.dirs[id as usize].children -= 1;
        if id == 0 || self.dirs[id as usize].children > 0 {
            return;
        }
        let mut scratch = String::new();
        let key = self.write_dir_key(id, &mut scratch).to_string();
        let hash = self.hash(&key);
        let at = self
            .dir_table
            .find(hash, |stored| stored == id)
            .expect("a live directory is in its table");
        self.dir_table.remove_at(at);
        let dir = self.dirs[id as usize];
        self.garbage += dir.path_len as usize;
        if dir.extra != NONE {
            self.garbage += block_len(self.text.from(dir.extra));
        }
        self.dirs[id as usize] = Dir::default();
        self.free_dirs.push(id);
    }

    // ----- entries -----

    /// Find an entry by its identity key.
    pub fn get(&self, key: &str) -> Option<EntryRef<'_>> {
        self.find(key).map(|(_, id)| EntryRef { catalog: self, id })
    }
    pub fn contains_key(&self, key: &str) -> bool {
        self.find(key).is_some()
    }
    /// Returns (table slot, entry id).
    fn find(&self, key: &str) -> Option<(usize, u32)> {
        let mut scratch = String::new();
        let at = self.key_table.find(self.hash(key), |id| {
            (EntryRef { catalog: self, id }).write_key(&mut scratch) == key
        })?;
        Some((at, self.key_table.slots[at]))
    }

    /// Insert a new entry or replace the one with the same key. Returns the id.
    pub fn insert(&mut self, input: &EntryInput<'_>) -> u32 {
        self.remove(input.key);
        self.insert_new(input)
    }
    /// Insert an entry whose key is known to be absent (bulk builds of a fresh
    /// catalog). Skips the lookup that `insert` needs for replacement.
    pub fn insert_new(&mut self, input: &EntryInput<'_>) -> u32 {
        debug_assert!(
            !self.contains_key(input.key),
            "insert_new with an existing key"
        );
        let split = Split::new(input);
        let dir = self.intern_dir(&split);
        // The shared record keeps its first child's display directory. Windows
        // keys are case-insensitive, so a sibling may spell the same directory
        // differently (`c:\docs`, `C:/Docs`); keep that spelling per entry.
        let own_dir =
            (!split.top && self.dir_path(dir) != split.dir_path).then_some(split.dir_path);
        let name = self.text.push(split.name_path.as_bytes());
        let mut flags = 0;
        if input.is_dir {
            flags |= IS_DIR;
        }
        if split.backslash {
            flags |= BACKSLASH;
        }
        if split.top {
            flags |= TOP;
        }
        if input.modified_ms.is_some() {
            flags |= HAS_TIME;
        }
        if !input.has_name {
            flags |= NAMELESS;
        }
        // Derivable when the key name is the ASCII-lowercased display name and
        // nothing else differs from it (true for most Windows paths).
        let derived = split.name_path.is_ascii()
            && split.name_key.len() == split.name_path.len()
            && split.name_key == split.name_path.to_ascii_lowercase()
            && split.name_search.is_none()
            && split.name_initials.is_none()
            && split.name_keyboard.is_none()
            && own_dir.is_none();
        let extra = if derived {
            flags |= DERIVED;
            NONE
        } else {
            self.text.put_block([
                Some(split.name_key),
                split.name_search,
                split.name_initials,
                split.name_keyboard,
                own_dir,
            ])
        };
        let node = Node {
            dir,
            name,
            extra,
            next: NONE,
            name_len: u16::try_from(split.name_path.len()).expect("a file name below 64 KiB"),
            flags,
            seen: input.seen as u32,
            size: input.size,
            modified: input.modified_ms.unwrap_or(0),
        };
        let id = if self.free_nodes != NONE {
            let id = self.free_nodes;
            self.free_nodes = self.nodes[id as usize].next;
            self.nodes[id as usize] = node;
            id
        } else {
            self.nodes.push(node);
            u32::try_from(self.nodes.len() - 1).expect("fewer than 4 billion entries")
        };
        let hash = self.hash(input.key);
        let mut table = std::mem::take(&mut self.key_table);
        table.insert(hash, id, |stored| {
            let mut scratch = String::new();
            self.hash(
                (EntryRef {
                    catalog: self,
                    id: stored,
                })
                .write_key(&mut scratch),
            )
        });
        self.key_table = table;
        self.len += 1;
        self.insertions += 1;
        id
    }

    /// Remove an entry by key. Returns true when it existed.
    pub fn remove(&mut self, key: &str) -> bool {
        let Some((at, id)) = self.find(key) else {
            return false;
        };
        self.key_table.remove_at(at);
        self.free_node(id);
        true
    }
    fn free_node(&mut self, id: u32) {
        let node = self.nodes[id as usize];
        self.garbage += node.name_len as usize;
        if node.extra != NONE {
            self.garbage += block_len(self.text.from(node.extra));
        }
        self.release_dir(node.dir);
        self.nodes[id as usize] = Node {
            dir: NONE,
            name: 0,
            extra: NONE,
            next: self.free_nodes,
            name_len: 0,
            flags: 0,
            seen: 0,
            size: 0,
            modified: 0,
        };
        self.free_nodes = id;
        self.len -= 1;
        if self.garbage > 64 * 1024 && self.garbage > self.text.used / 2 {
            self.compact();
        }
    }

    /// Keep only entries for which `keep` returns true.
    pub fn retain(&mut self, mut keep: impl FnMut(EntryRef<'_>) -> bool) {
        let doomed: Vec<u32> = self
            .iter()
            .filter(|entry| !keep(*entry))
            .map(|entry| entry.id)
            .collect();
        let mut scratch = String::new();
        for id in doomed {
            let key = (EntryRef { catalog: self, id })
                .write_key(&mut scratch)
                .to_string();
            let (at, _) = self.find(&key).expect("a live entry is in its table");
            self.key_table.remove_at(at);
            self.free_node(id);
        }
    }

    #[allow(dead_code)] // Kept for in-place updates; covered by the module tests.
    /// Update metadata of an existing entry in place.
    pub fn set_metadata(&mut self, id: u32, size: u64, modified_ms: Option<i64>, seen: u64) {
        let node = &mut self.nodes[id as usize];
        node.size = size;
        node.modified = modified_ms.unwrap_or(0);
        node.flags = (node.flags & !HAS_TIME) | if modified_ms.is_some() { HAS_TIME } else { 0 };
        node.seen = seen as u32;
    }
    pub fn set_seen(&mut self, id: u32, seen: u64) {
        self.nodes[id as usize].seen = seen as u32;
    }

    /// Number of entry slots (live or free). Ids are below this bound, so a
    /// scan can split `0..slots()` between threads without collecting ids.
    pub fn slots(&self) -> usize {
        self.nodes.len()
    }
    /// The live entry with this id, or `None` for a free or unknown slot.
    pub fn entry(&self, id: u32) -> Option<EntryRef<'_>> {
        self.nodes
            .get(id as usize)
            .filter(|node| node.dir != NONE)
            .map(|_| EntryRef { catalog: self, id })
    }
    /// All live entries, in storage order.
    pub fn iter(&self) -> impl Iterator<Item = EntryRef<'_>> + '_ {
        (0..self.nodes.len() as u32)
            .filter(|&id| self.nodes[id as usize].dir != NONE)
            .map(|id| EntryRef { catalog: self, id })
    }

    /// The entry at `root` and every entry below it (`root/...`).
    pub fn subtree<'a>(&'a self, root: &'a str) -> impl Iterator<Item = EntryRef<'a>> + 'a {
        // Decide once per directory whether it is the root or below it.
        let mut scratch = String::new();
        let inside: Vec<bool> = (0..self.dirs.len() as u32)
            .map(|id| {
                id != 0
                    && self.dirs[id as usize].children > 0
                    && under(self.write_dir_key(id, &mut scratch), root)
            })
            .collect();
        self.iter().filter(move |entry| {
            if inside[entry.node().dir as usize] {
                return true;
            }
            // The root entry itself lives in its parent directory.
            entry.write_key(&mut scratch) == root
        })
    }

    /// Rewrite the text arena without removed names, paths and blocks.
    fn compact(&mut self) {
        let mut text = Text::default();
        text.reserve(self.text.used.saturating_sub(self.garbage));
        for dir in self.dirs.iter_mut().skip(1).filter(|dir| dir.children > 0) {
            dir.path = text.push(self.text.bytes(dir.path, dir.path_len as usize));
            if dir.extra != NONE {
                dir.extra = text.push(self.text.block(dir.extra));
            }
        }
        for node in self.nodes.iter_mut().filter(|node| node.dir != NONE) {
            node.name = text.push(self.text.bytes(node.name, node.name_len as usize));
            if node.extra != NONE {
                node.extra = text.push(self.text.block(node.extra));
            }
        }
        self.text = text;
        self.garbage = 0;
    }

    // Also used by tools/catalog-qa (memory benchmark and diagnostics).
    #[allow(dead_code)]
    /// Per-part memory breakdown for diagnostics and the memory benchmark.
    pub fn breakdown(&self) -> Vec<(&'static str, usize)> {
        vec![
            ("entries", self.len),
            (
                "dirs",
                self.dirs
                    .iter()
                    .skip(1)
                    .filter(|dir| dir.children > 0)
                    .count(),
            ),
            (
                "dirRecordBytes",
                self.dirs.capacity() * std::mem::size_of::<Dir>(),
            ),
            ("nodeBytes", self.nodes.capacity_bytes()),
            ("textBytes", self.text.capacity()),
            ("dirTableBytes", self.dir_table.bytes()),
            ("keyTableBytes", self.key_table.bytes()),
        ]
    }
    // Also used by tools/catalog-qa (memory benchmark and diagnostics).
    #[allow(dead_code)]
    /// Heap bytes owned by the catalog.
    pub fn heap_bytes(&self) -> usize {
        self.dirs.capacity() * std::mem::size_of::<Dir>()
            + self.free_dirs.capacity() * 4
            + self.nodes.capacity_bytes()
            + self.text.capacity()
            + self.dir_table.bytes()
            + self.key_table.bytes()
    }
}

fn under(path: &str, root: &str) -> bool {
    path == root
        || path
            .strip_prefix(root)
            .is_some_and(|tail| tail.starts_with('/'))
}

/// A whole-path input split into its directory part and its own name part.
struct Split<'a> {
    top: bool,
    backslash: bool,
    dir_path: &'a str,
    dir_key: &'a str,
    dir_search: Option<&'a str>,
    dir_initials: Option<&'a str>,
    dir_keyboard: Option<&'a str>,
    name_path: &'a str,
    name_key: &'a str,
    name_search: Option<&'a str>,
    name_initials: Option<&'a str>,
    name_keyboard: Option<&'a str>,
}
impl<'a> Split<'a> {
    fn new(input: &EntryInput<'a>) -> Self {
        // The key decides the directory: `c:/docs/a.txt` -> `c:/docs` + `a.txt`.
        let Some(key_at) = input.key.rfind('/') else {
            // A top-level entry such as the drive root `c:` (display `C:\`).
            return Self {
                top: true,
                backslash: false,
                dir_path: "",
                dir_key: "",
                dir_search: None,
                dir_initials: None,
                dir_keyboard: None,
                name_path: input.path,
                name_key: input.key,
                name_search: input.search,
                name_initials: input.initials,
                name_keyboard: input.keyboard,
            };
        };
        // The display path splits at its last separator. Display and key name
        // the same components, so this is the same boundary. Keys never end in
        // a separator, but a display path may (`\\server\share\`): ignore those
        // trailing separators so the directory part matches the key's, and keep
        // them in the name part so the display path is still rebuilt exactly.
        let path_at = input
            .path
            .trim_end_matches(['\\', '/'])
            .rfind(['\\', '/'])
            .expect("a nested key has a separated display path");
        let backslash = input.path.as_bytes()[path_at] == b'\\';
        // Each derived text is split at its own last '/'. The keyboard
        // projection maps Hangul to Latin keys, so its byte offsets differ from
        // the search text; names never contain '/' in any projection.
        let cut = |text: Option<&'a str>| -> (Option<&'a str>, Option<&'a str>) {
            text.map(|text| {
                let at = text
                    .rfind('/')
                    .expect("a nested key has separated derived text");
                (&text[..at], &text[at + 1..])
            })
            .unzip()
        };
        let (dir_search, name_search) = cut(input.search);
        let (dir_initials, name_initials) = cut(input.initials);
        let (dir_keyboard, name_keyboard) = cut(input.keyboard);
        Self {
            top: false,
            backslash,
            dir_path: &input.path[..path_at],
            dir_key: &input.key[..key_at],
            dir_search,
            dir_initials,
            dir_keyboard,
            name_path: &input.path[path_at + 1..],
            name_key: &input.key[key_at + 1..],
            name_search,
            name_initials,
            name_keyboard,
        }
    }
}

/// Reusable buffer for scans. Entries are stored roughly in directory order,
/// so consecutive entries usually share a directory; the cache keeps that
/// directory's search prefix and only replaces the name part.
#[derive(Default)]
pub struct SearchScratch {
    buffer: String,
    /// (directory id, prefix length) of the cached prefix, if any.
    cached: Option<(u32, usize)>,
}

impl<'a> EntryRef<'a> {
    /// Search text, like `write_search`, reusing the cached directory prefix.
    pub fn search_text<'b>(&self, scratch: &'b mut SearchScratch) -> &'b str {
        if self.top() {
            scratch.cached = None;
            return self.write_search(&mut scratch.buffer);
        }
        let dir = self.node().dir;
        match scratch.cached {
            Some((cached, len)) if cached == dir => scratch.buffer.truncate(len),
            _ => {
                scratch.buffer.clear();
                match self.catalog.dir_part(dir, 1) {
                    Some(text) => {
                        scratch.buffer.push_str(text);
                        scratch.buffer.push('/');
                    }
                    None => self.push_dir_key(&mut scratch.buffer),
                }
                scratch.cached = Some((dir, scratch.buffer.len()));
            }
        }
        match self.stored(1) {
            Some(name) => scratch.buffer.push_str(name),
            None => self.push_key_name(&mut scratch.buffer),
        }
        scratch.buffer.as_str()
    }

    fn node(&self) -> &'a Node {
        &self.catalog.nodes[self.id as usize]
    }
    pub fn id(&self) -> u32 {
        self.id
    }
    /// Whether the entry ranks by its file name (`EntryInput::has_name`).
    pub fn has_name(&self) -> bool {
        self.node().flags & NAMELESS == 0
    }
    pub fn is_dir(&self) -> bool {
        self.node().flags & IS_DIR != 0
    }
    pub fn size(&self) -> u64 {
        self.node().size
    }
    pub fn modified_ms(&self) -> Option<i64> {
        let node = self.node();
        (node.flags & HAS_TIME != 0).then_some(node.modified)
    }
    /// The stored sweep id (modulo 2^32).
    pub fn seen(&self) -> u32 {
        self.node().seen
    }
    /// Whether the entry was refreshed by `sweep`.
    pub fn is_seen(&self, sweep: u64) -> bool {
        self.node().seen == sweep as u32
    }
    /// The display name exactly as it appears in the display path.
    pub fn display_name(&self) -> &'a str {
        let node = self.node();
        self.catalog.text_str(node.name, node.name_len as usize)
    }
    /// Stored name part `index` (0 key, 1 search, 2 initials, 3 keyboard).
    fn stored(&self, index: usize) -> Option<&'a str> {
        let node = self.node();
        (node.flags & DERIVED == 0)
            .then(|| get_block(self.catalog.text.from(node.extra), index))
            .flatten()
    }
    fn top(&self) -> bool {
        self.node().flags & TOP != 0
    }
    /// `dir` + separator + `name`, or just `name` for top-level entries.
    fn join<'b>(&self, buffer: &'b mut String, dir: &str, separator: char, name: &str) -> &'b str {
        buffer.clear();
        if !self.top() {
            buffer.push_str(dir);
            buffer.push(separator);
        }
        buffer.push_str(name);
        buffer.as_str()
    }
    /// Write the display path, for example `C:\Docs\a.txt`.
    pub fn write_path<'b>(&self, buffer: &'b mut String) -> &'b str {
        let separator = if self.node().flags & BACKSLASH != 0 {
            '\\'
        } else {
            '/'
        };
        let dir = if self.top() {
            ""
        } else {
            self.stored(OWN_DIR)
                .unwrap_or_else(|| self.catalog.dir_path(self.node().dir))
        };
        self.join(buffer, dir, separator, self.display_name())
    }
    /// Append the directory key (when nested) and a `/`.
    fn push_dir_key(&self, buffer: &mut String) {
        if self.top() {
            return;
        }
        let dir = self.node().dir;
        match self.catalog.dir_part(dir, 0) {
            Some(key) => buffer.push_str(key),
            None => derive_key(self.catalog.dir_path(dir), buffer),
        }
        buffer.push('/');
    }
    /// Append the key name.
    fn push_key_name(&self, buffer: &mut String) {
        match self.stored(0) {
            Some(name) => buffer.push_str(name),
            None => {
                let start = buffer.len();
                buffer.push_str(self.display_name());
                buffer[start..].make_ascii_lowercase();
            }
        }
    }
    /// Write the identity key, for example `c:/docs/a.txt`.
    pub fn write_key<'b>(&self, buffer: &'b mut String) -> &'b str {
        buffer.clear();
        self.push_dir_key(buffer);
        self.push_key_name(buffer);
        buffer.as_str()
    }
    /// Whether the original entry had its own search text (`search` field).
    pub fn has_search(&self) -> bool {
        self.stored(1).is_some()
            || (!self.top() && self.catalog.dir_part(self.node().dir, 1).is_some())
    }
    /// Write the search text (identical to the key unless `has_search`).
    pub fn write_search<'b>(&self, buffer: &'b mut String) -> &'b str {
        buffer.clear();
        let dir_search = (!self.top())
            .then(|| self.catalog.dir_part(self.node().dir, 1))
            .flatten();
        match dir_search {
            Some(dir) => {
                buffer.push_str(dir);
                buffer.push('/');
            }
            None => self.push_dir_key(buffer),
        }
        match self.stored(1) {
            Some(name) => buffer.push_str(name),
            None => self.push_key_name(buffer),
        }
        buffer.as_str()
    }
    fn projection<'b>(&self, buffer: &'b mut String, index: usize) -> Option<&'b str> {
        // An entry has a projection exactly when its own name part was stored.
        let name = self.stored(index)?;
        buffer.clear();
        if !self.top() {
            let dir = self.node().dir;
            match self
                .catalog
                .dir_part(dir, index)
                .or_else(|| self.catalog.dir_part(dir, 1))
            {
                // Without its own projection, the directory part equals its search text.
                Some(text) => buffer.push_str(text),
                None => {
                    self.push_dir_key(buffer);
                    buffer.pop();
                }
            }
            buffer.push('/');
        }
        buffer.push_str(name);
        Some(buffer.as_str())
    }
    /// Write the Hangul initials projection when the entry has one.
    pub fn write_initials<'b>(&self, buffer: &'b mut String) -> Option<&'b str> {
        self.projection(buffer, 2)
    }
    /// Write the keyboard projection when the entry has one.
    pub fn write_keyboard<'b>(&self, buffer: &'b mut String) -> Option<&'b str> {
        self.projection(buffer, 3)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input<'a>(path: &'a str, key: &'a str) -> EntryInput<'a> {
        EntryInput {
            path,
            key,
            search: None,
            initials: None,
            keyboard: None,
            has_name: !path.ends_with(['\\', '/']),
            is_dir: false,
            size: 7,
            modified_ms: Some(11),
            seen: 3,
        }
    }

    /// Check that every stored string is rebuilt exactly.
    fn assert_round_trip(catalog: &Catalog, expected: &EntryInput<'_>) {
        let entry = catalog.get(expected.key).expect("entry exists");
        let mut buffer = String::new();
        assert_eq!(entry.write_path(&mut buffer), expected.path);
        assert_eq!(entry.write_key(&mut buffer), expected.key);
        assert_eq!(
            entry.write_search(&mut buffer),
            expected.search.unwrap_or(expected.key)
        );
        assert_eq!(entry.write_initials(&mut buffer), expected.initials);
        assert_eq!(entry.write_keyboard(&mut buffer), expected.keyboard);
        assert_eq!(entry.has_name(), expected.has_name);
        assert_eq!(entry.is_dir(), expected.is_dir);
        assert_eq!(entry.size(), expected.size);
        assert_eq!(entry.modified_ms(), expected.modified_ms);
        assert!(entry.is_seen(expected.seen));
    }

    #[test]
    fn text_items_never_straddle_chunks_and_oversized_items_get_their_own_chunk() {
        let mut text = Text::default();
        // Fill past several 4 MiB chunks with odd-sized items so some items
        // would cross a boundary if placement allowed it.
        let mut stored = Vec::new();
        let mut total = 0usize;
        let mut number = 0usize;
        while total < 3 * TEXT_CHUNK + 12_345 {
            let item = format!("item-{number}-{}", "x".repeat(number % 997));
            stored.push((text.push(item.as_bytes()), item.clone()));
            total += item.len();
            number += 1;
        }
        // A zero-length item and an item larger than a chunk.
        stored.push((text.push(b""), String::new()));
        let huge = "가".repeat(TEXT_CHUNK / 3 + 7);
        assert!(huge.len() > TEXT_CHUNK);
        stored.push((text.push(huge.as_bytes()), huge.clone()));
        let block = text.put_block([Some("key"), None, Some("초성"), Some(""), None]);
        stored.push((text.push(b"after"), "after".to_string()));
        assert!(text.chunks.len() >= 5);
        for chunk in &text.chunks {
            assert!(chunk.len() <= TEXT_CHUNK || chunk.len() == huge.len());
        }
        for (offset, expected) in &stored {
            assert_eq!(text.bytes(*offset, expected.len()), expected.as_bytes());
        }
        let read = text.from(block);
        assert_eq!(get_block(read, 0), Some("key"));
        assert_eq!(get_block(read, 1), None);
        assert_eq!(get_block(read, 2), Some("초성"));
        assert_eq!(get_block(read, 3), Some(""));
        assert_eq!(
            text.block(block).len(),
            block_size(&[Some("key"), None, Some("초성"), Some(""), None])
        );
        // Shrinking keeps every item readable.
        text.shrink_to_fit();
        for (offset, expected) in &stored {
            assert_eq!(text.bytes(*offset, expected.len()), expected.as_bytes());
        }
    }

    #[test]
    fn many_entries_span_node_and_text_chunks_and_still_round_trip() {
        let mut catalog = Catalog::new();
        // 150,000 entries: three node chunks and more than one text chunk.
        let count = 2 * NODE_CHUNK + 18_000;
        let rows: Vec<(String, String)> = (0..count)
            .map(|index| {
                let path = format!(
                    r"C:\Volume\폴더{}\깊은-경로-{}\파일-{index:07}-이름이-조금-긴-문서.txt",
                    index % 211,
                    index % 7
                );
                let key = path.replace('\\', "/").to_lowercase();
                (path, key)
            })
            .collect();
        for (path, key) in &rows {
            catalog.insert_new(&input(path, key));
        }
        assert!(catalog.nodes.chunks.len() == 3);
        assert!(catalog.text.chunks.len() > 1);
        for (path, key) in rows.iter().step_by(97) {
            assert_round_trip(&catalog, &input(path, key));
        }
        catalog.shrink_to_fit();
        // Removing more than half compacts the text; survivors still round trip.
        for (_, key) in rows.iter().step_by(2) {
            assert!(catalog.remove(key));
        }
        // Odd indices survived; an even step keeps the sample on them.
        for (path, key) in rows.iter().skip(1).step_by(54) {
            assert_round_trip(&catalog, &input(path, key));
        }
        assert_eq!(catalog.len(), count / 2);
    }

    #[test]
    fn windows_ascii_paths_round_trip_and_share_their_directory() {
        let mut catalog = Catalog::new();
        let entries = [
            input(r"C:\", "c:"),
            input(r"C:\Windows", "c:/windows"),
            input(
                r"C:\Windows\System32\Drivers.SYS",
                "c:/windows/system32/drivers.sys",
            ),
            input(
                r"C:\Windows\System32\cmd.exe",
                "c:/windows/system32/cmd.exe",
            ),
        ];
        for entry in &entries {
            catalog.insert(entry);
        }
        assert_eq!(catalog.len(), 4);
        for entry in &entries {
            assert_round_trip(&catalog, entry);
        }
        // Directory records exist only for parents with entries: `c:` (parent of
        // `c:/windows`) and `c:/windows/system32`, shared by both of its files.
        assert_eq!(catalog.dir_table.used, 2);
    }

    #[test]
    fn unix_and_unc_paths_round_trip() {
        let mut catalog = Catalog::new();
        let entries = [
            input("/home/User/Notes.md", "/home/User/Notes.md"),
            input("/", ""),
            input(r"\\server\share\Report.pdf", "//server/share/report.pdf"),
        ];
        // An empty key is a degenerate root; it must still round trip.
        for entry in &entries {
            catalog.insert(entry);
        }
        for entry in &entries {
            assert_round_trip(&catalog, entry);
        }
    }

    #[test]
    fn hangul_search_text_and_projections_round_trip() {
        let mut catalog = Catalog::new();
        // Projections keep the search text's byte length (3-byte syllable -> 3-byte jamo).
        let search = "c:/문서함/공문서 계획.hwpx";
        let initials = "c:/ㅁㅅㅎ/ㄱㅁㅅ ㄱㅎ.hwpx";
        let keyboard = "c:/anstjgka/rhdanstj rPghlr.hwpx";
        let mut entry = input(r"C:\문서함\공문서 계획.HWPX", "c:/문서함/공문서 계획.hwpx");
        entry.search = Some(search);
        entry.initials = Some(initials);
        entry.keyboard = Some(keyboard);
        catalog.insert(&entry);
        assert_round_trip(&catalog, &entry);
        // A sibling with an ASCII name under the same Hangul directory.
        let mut sibling = input(r"C:\문서함\plan.txt", "c:/문서함/plan.txt");
        sibling.search = Some("c:/문서함/plan.txt");
        sibling.initials = Some("c:/ㅁㅅㅎ/plan.txt");
        catalog.insert(&sibling);
        assert_round_trip(&catalog, &sibling);
    }

    #[test]
    fn a_hangul_name_after_an_ascii_sibling_keeps_its_projections() {
        let mut catalog = Catalog::new();
        let first = input(r"C:\Docs\plan.txt", "c:/docs/plan.txt");
        catalog.insert(&first);
        let mut later = input(r"C:\Docs\계획.txt", "c:/docs/계획.txt");
        later.search = Some("c:/docs/계획.txt");
        later.initials = Some("c:/docs/ㄱㅎ.txt");
        later.keyboard = Some("c:/docs/rPghlr.txt");
        catalog.insert(&later);
        assert_round_trip(&catalog, &first);
        assert_round_trip(&catalog, &later);
        // And the reverse order under a Hangul directory.
        let mut hangul = input(r"C:\문서\계획.txt", "c:/문서/계획.txt");
        hangul.initials = Some("c:/ㅁㅅ/ㄱㅎ.txt");
        hangul.keyboard = Some("c:/anstj/rPghlr.txt");
        let mut ascii = input(r"C:\문서\plan.txt", "c:/문서/plan.txt");
        ascii.initials = Some("c:/ㅁㅅ/plan.txt");
        ascii.keyboard = Some("c:/anstj/plan.txt");
        catalog.insert(&ascii);
        catalog.insert(&hangul);
        assert_round_trip(&catalog, &ascii);
        assert_round_trip(&catalog, &hangul);
    }

    #[test]
    fn replace_remove_retain_and_subtree() {
        let mut catalog = Catalog::new();
        for (path, key) in [
            (r"C:\a", "c:/a"),
            (r"C:\a\x.txt", "c:/a/x.txt"),
            (r"C:\a\b\y.txt", "c:/a/b/y.txt"),
            (r"C:\ab\z.txt", "c:/ab/z.txt"),
        ] {
            catalog.insert(&input(path, key));
        }
        // Replacing keeps one entry and updates its fields.
        let mut changed = input(r"C:\a\x.txt", "c:/a/x.txt");
        changed.size = 99;
        catalog.insert(&changed);
        assert_eq!(catalog.len(), 4);
        assert_eq!(catalog.get("c:/a/x.txt").unwrap().size(), 99);
        // The subtree excludes the sibling prefix `c:/ab`.
        let mut buffer = String::new();
        let mut keys: Vec<String> = catalog
            .subtree("c:/a")
            .map(|entry| entry.write_key(&mut buffer).to_string())
            .collect();
        keys.sort();
        assert_eq!(keys, ["c:/a", "c:/a/b/y.txt", "c:/a/x.txt"]);
        assert!(catalog.remove("c:/a/b/y.txt"));
        assert!(!catalog.remove("c:/a/b/y.txt"));
        assert!(
            catalog.find_dir("c:/a/b").is_none(),
            "empty directory record released"
        );
        catalog.retain(|entry| entry.size() != 99);
        assert!(!catalog.contains_key("c:/a/x.txt"));
        assert_eq!(catalog.len(), 2);
    }

    #[test]
    fn removed_slots_and_names_are_reused_and_compacted() {
        let mut catalog = Catalog::new();
        let paths: Vec<(String, String)> = (0..20_000)
            .map(|index| {
                let path = format!(r"C:\Data\Folder{}\File-{index:06}.bin", index % 50);
                let key = path.replace('\\', "/").to_ascii_lowercase();
                (path, key)
            })
            .collect();
        for (path, key) in &paths {
            catalog.insert(&input(path, key));
        }
        for (_, key) in paths.iter().step_by(2) {
            assert!(catalog.remove(key));
        }
        // More than half the names were garbage: the arena has been compacted.
        assert!(catalog.text.used < 20_000 * 16);
        for (path, key) in paths.iter().skip(1).step_by(2) {
            assert_round_trip(&catalog, &input(path, key));
        }
        let nodes = catalog.nodes.len();
        catalog.insert(&input(&paths[0].0, &paths[0].1));
        assert_eq!(catalog.nodes.len(), nodes, "a freed node slot is reused");
    }

    /// Review counterexamples: sibling spellings of one directory key, and the
    /// extreme modification time, must all come back exactly.
    #[test]
    fn sibling_directory_spellings_and_extreme_times_round_trip() {
        let mut catalog = Catalog::new();
        let mut entries = vec![
            input(r"C:\Docs\one.txt", "c:/docs/one.txt"),
            input(r"c:\docs\two.txt", "c:/docs/two.txt"),
            input("C:/Docs/three.txt", "c:/docs/three.txt"),
            input(r"C:\Docs/four.txt", "c:/docs/four.txt"),
        ];
        entries[1].modified_ms = Some(i64::MIN);
        entries[2].modified_ms = Some(i64::MAX);
        entries[3].modified_ms = None;
        for entry in &entries {
            catalog.insert(entry);
        }
        for entry in &entries {
            assert_round_trip(&catalog, entry);
        }
        // Releasing the first spelling must not change the others.
        assert!(catalog.remove("c:/docs/one.txt"));
        for entry in &entries[1..] {
            assert_round_trip(&catalog, entry);
        }
        // Metadata updates keep the presence of a time exact too.
        let id = catalog.get("c:/docs/two.txt").unwrap().id();
        catalog.set_metadata(id, 1, None, 2);
        assert_eq!(catalog.get("c:/docs/two.txt").unwrap().modified_ms(), None);
        catalog.set_metadata(id, 1, Some(i64::MIN), 2);
        assert_eq!(
            catalog.get("c:/docs/two.txt").unwrap().modified_ms(),
            Some(i64::MIN)
        );
    }

    #[test]
    fn trailing_separator_roots_keep_their_siblings_paths() {
        let mut catalog = Catalog::new();
        let entries = [
            input(r"\\server\share\", "//server/share"),
            input(r"\\server\other", "//server/other"),
            input(r"\\server\share\a.txt", "//server/share/a.txt"),
            input(r"C:\", "c:"),
            input("/srv/data/", "/srv/data"),
            input("/srv/logs", "/srv/logs"),
        ];
        // Insert in both orders: the first child decides the directory record.
        for order in [true, false] {
            catalog.clear();
            let list: Vec<_> = if order {
                entries.iter().collect()
            } else {
                entries.iter().rev().collect()
            };
            for entry in list {
                catalog.insert(entry);
            }
            for entry in &entries {
                assert_round_trip(&catalog, entry);
            }
        }
        let names: Vec<bool> = entries
            .iter()
            .map(|entry| catalog.get(entry.key).unwrap().has_name())
            .collect();
        assert_eq!(names, [false, true, true, false, false, true]);
    }

    #[test]
    fn cached_search_text_matches_write_search() {
        let mut catalog = Catalog::new();
        let mut hangul = input(r"C:\문서\계획.txt", "c:/문서/계획.txt");
        hangul.search = Some("c:/문서/계획.txt");
        for entry in [
            input(r"C:\", "c:"),
            input(r"C:\Docs\A.txt", "c:/docs/a.txt"),
            input(r"C:\Docs\B.txt", "c:/docs/b.txt"),
            hangul,
            input(r"C:\Docs\C.txt", "c:/docs/c.txt"),
        ] {
            catalog.insert(&entry);
        }
        let mut scratch = SearchScratch::default();
        let mut plain = String::new();
        for entry in catalog.iter() {
            let expected = entry.write_search(&mut plain).to_string();
            assert_eq!(entry.search_text(&mut scratch), expected);
        }
    }

    /// Random inserts, replacements, removals and retains, checked against a
    /// plain map of the original whole strings after every step batch.
    #[test]
    fn random_operations_match_a_reference_map() {
        use std::collections::BTreeMap;
        let mut state: u64 = 0x2545_F491_4F6C_DD1D;
        let mut next = move |limit: u64| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state % limit
        };
        // Owned whole strings per key: (path, search, initials, keyboard, size, seen).
        type Row = (
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            u64,
            u64,
        );
        let mut reference: BTreeMap<String, Row> = BTreeMap::new();
        let mut catalog = Catalog::new();
        let folders = ["Docs", "문서", "Data", "MiXeD", "ㄱ"];
        let names = ["a.txt", "B.TXT", "계획.hwpx", "readme", "x y.md", "가나"];
        for step in 0..30_000u64 {
            let depth = 1 + next(3) as usize;
            let mut path = String::from("C:");
            for _ in 0..depth {
                // The same directory key may be spelled with either separator
                // and any case; each spelling must round trip per entry.
                path.push(if next(4) == 0 { '/' } else { '\\' });
                let folder = folders[next(folders.len() as u64) as usize];
                if next(4) == 0 {
                    path.push_str(&folder.to_lowercase());
                } else {
                    path.push_str(folder);
                }
            }
            path.push('\\');
            path.push_str(names[next(names.len() as u64) as usize]);
            let key = path.replace('\\', "/").to_lowercase();
            match next(10) {
                0..=5 => {
                    // Non-ASCII paths get search text and projections (any
                    // deterministic function of the key works for storage).
                    let hangul = !key.is_ascii();
                    let search = hangul.then(|| key.clone());
                    let initials =
                        hangul.then(|| key.replace("문서", "ㅁㅅ").replace("계획", "ㄱㅎ"));
                    let keyboard =
                        hangul.then(|| key.replace("문서", "anstj").replace("계획", "rPghlr"));
                    let size = next(1000);
                    catalog.insert(&EntryInput {
                        path: &path,
                        key: &key,
                        search: search.as_deref(),
                        initials: initials.as_deref(),
                        keyboard: keyboard.as_deref(),
                        has_name: true,
                        is_dir: false,
                        size,
                        modified_ms: None,
                        seen: step,
                    });
                    reference.insert(key, (path, search, initials, keyboard, size, step));
                }
                6..=8 => {
                    assert_eq!(catalog.remove(&key), reference.remove(&key).is_some());
                }
                _ => {
                    let cut = next(1000);
                    catalog.retain(|entry| entry.size() >= cut);
                    reference.retain(|_, row| row.4 >= cut);
                }
            }
            if step % 997 == 0 || step == 29_999 {
                assert_eq!(catalog.len(), reference.len());
                for (key, row) in &reference {
                    let expected = EntryInput {
                        path: &row.0,
                        key,
                        search: row.1.as_deref(),
                        initials: row.2.as_deref(),
                        keyboard: row.3.as_deref(),
                        has_name: true,
                        is_dir: false,
                        size: row.4,
                        modified_ms: None,
                        seen: row.5,
                    };
                    assert_round_trip(&catalog, &expected);
                }
                assert_eq!(catalog.iter().count(), reference.len());
                let mut scratch = SearchScratch::default();
                let mut plain = String::new();
                for entry in catalog.iter() {
                    let expected = entry.write_search(&mut plain).to_string();
                    assert_eq!(entry.search_text(&mut scratch), expected);
                }
            }
        }
    }
}

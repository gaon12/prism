//! Trusted Office runtime data. Catalogs are compiled in, never supplied by IPC.
//! An upstream MSI pin alone does not authorize executing an arbitrary folder.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub const WORKER: &[u8] = include_bytes!("../../tools/office-runtime/worker.py");
const CATALOG: &str = include_str!("../../tools/office-runtime/windows-x64.lock.json");
const OWNER: &str = ".prism-office-runtime";
pub const MAX_DOCUMENT: usize = 64 << 20;
pub type Result<T> = std::result::Result<T, Error>;
pub type Check<'a> = &'a dyn Fn() -> Result<()>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Code {
    Unavailable,
    Input,
    Integrity,
    Cancelled,
    TimedOut,
    Busy,
    Encrypted,
    Unsupported,
    Limit,
    Protocol,
    Failed,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Error {
    pub code: Code,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_code: Option<i32>,
}
pub fn error(code: Code) -> Error {
    Error {
        code,
        stage: None,
        native_code: None,
    }
}
fn io_error(stage: &'static str) -> impl FnOnce(std::io::Error) -> Error {
    move |error| Error {
        code: Code::Failed,
        stage: Some(stage),
        native_code: error.raw_os_error(),
    }
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Archive {
    pub bytes: u64,
    pub sha256: String,
    pub url: Option<String>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Catalog {
    pub schema: u32,
    pub target: String,
    pub engine: String,
    pub version: String,
    pub executable: String,
    pub engine_path: String,
    pub inventory_sha256: String,
    pub file_count: usize,
    pub expanded_bytes: u64,
    pub archive: Archive,
    pub upstream_msi_sha256: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}
pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub fn relative(value: &str) -> Result<&Path> {
    if value.is_empty()
        || value.len() > 2048
        || value
            .chars()
            .any(|c| c.is_control() || "\\:<>\"|?*".contains(c))
        || value.split('/').any(|part| {
            let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
            part.is_empty()
                || matches!(part, "." | "..")
                || part.ends_with(['.', ' '])
                || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                || ["COM", "LPT"].iter().any(|prefix| {
                    stem.strip_prefix(prefix).is_some_and(|tail| {
                        matches!(tail, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
                    })
                })
        })
    {
        return Err(error(Code::Input));
    }
    Ok(Path::new(value))
}
pub fn catalog() -> Result<Catalog> {
    let value: Catalog = serde_json::from_str(CATALOG).map_err(|_| error(Code::Integrity))?;
    value.validate()?;
    Ok(value)
}
impl Catalog {
    fn validate(&self) -> Result<()> {
        if self.schema != 1
            || self.target != "windows-x64"
            || self.engine != "LibreOffice"
            || self.version.is_empty()
            || self.version.len() > 64
            || !self
                .version
                .bytes()
                .all(|b| b.is_ascii_digit() || b == b'.')
            || !(1..=60_000).contains(&self.file_count)
            || self.expanded_bytes > 4 << 30
            || !(1..=1 << 30).contains(&self.archive.bytes)
            || !valid_hash(&self.archive.sha256)
            || !valid_hash(&self.inventory_sha256)
            || !valid_hash(&self.upstream_msi_sha256)
            || self.archive.url.as_ref().is_some_and(|url| {
                !url.starts_with("https://github.com/gaon12/prism/releases/download/")
                    || !url
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-._:/".contains(&b))
            })
        {
            return Err(error(Code::Integrity));
        }
        relative(&self.executable)?;
        relative(&self.engine_path)?;
        Ok(())
    }
    pub fn directory(&self) -> String {
        format!(
            "libreoffice-{}-{}",
            self.target,
            &self.inventory_sha256[..16]
        )
    }
    fn entries(&self, bytes: &[u8]) -> Result<Vec<Entry>> {
        if bytes.len() > 16 << 20 || digest(bytes) != self.inventory_sha256 {
            return Err(error(Code::Integrity));
        }
        let entries: Vec<Entry> =
            serde_json::from_slice(bytes).map_err(|_| error(Code::Integrity))?;
        let mut seen = HashSet::new();
        let mut sum = 0u64;
        let mut last: Option<&str> = None;
        for entry in &entries {
            relative(&entry.path)?;
            sum = sum
                .checked_add(entry.bytes)
                .ok_or_else(|| error(Code::Limit))?;
            if !seen.insert(entry.path.to_ascii_lowercase())
                || matches!(entry.path.as_str(), "inventory.json" | OWNER)
                || last.is_some_and(|last| entry.path.as_str() <= last)
                || entry.bytes > 512 << 20
                || !valid_hash(&entry.sha256)
            {
                return Err(error(Code::Integrity));
            }
            last = Some(&entry.path);
        }
        if entries.len() != self.file_count
            || sum != self.expanded_bytes
            || !seen.contains(&self.executable.to_ascii_lowercase())
            || !seen.contains(&self.engine_path.to_ascii_lowercase())
        {
            return Err(error(Code::Integrity));
        }
        Ok(entries)
    }
}
fn not_link(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return false;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return false;
        }
    }
    true
}
fn leaf(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1).custom_flags(0x00200000);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let file = options.open(path).map_err(|_| error(Code::Integrity))?;
    let metadata = file.metadata().map_err(|_| error(Code::Integrity))?;
    if !metadata.is_file() || !not_link(&metadata) {
        return Err(error(Code::Integrity));
    }
    Ok(file)
}
pub fn regular(path: &Path) -> Result<File> {
    crate::path_identity::no_links(path).map_err(|_| error(Code::Integrity))?;
    leaf(path)
}
pub(crate) fn mkdir(path: &Path) -> Result<()> {
    // comparison checks the existing ancestors before creating a missing suffix.
    // no_links alone requires the final directory to exist already.
    let checked = crate::path_identity::comparison(path).map_err(|_| error(Code::Integrity))?;
    fs::create_dir_all(&checked).map_err(io_error("createDirectory"))?;
    crate::path_identity::no_links(&checked).map_err(|_| error(Code::Integrity))
}
pub fn read_hash(mut reader: impl Read, limit: u64, check: Check<'_>) -> Result<(u64, String)> {
    let mut count = 0u64;
    let mut hasher = Sha256::new();
    let mut buffer = [0; 64 << 10];
    loop {
        check()?;
        let n = reader
            .read(&mut buffer)
            .map_err(|_| error(Code::Integrity))?;
        if n == 0 {
            break;
        }
        count = count
            .checked_add(n as u64)
            .ok_or_else(|| error(Code::Limit))?;
        if count > limit {
            return Err(error(Code::Limit));
        }
        hasher.update(&buffer[..n]);
    }
    Ok((count, format!("{:x}", hasher.finalize())))
}
fn read_small(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    regular(path)?
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| error(Code::Integrity))?;
    if bytes.len() as u64 > limit {
        return Err(error(Code::Limit));
    }
    Ok(bytes)
}
pub fn verify(
    root: &Path,
    catalog: &Catalog,
    check: Check<'_>,
    progress: &dyn Fn(usize),
) -> Result<()> {
    catalog.validate()?;
    crate::path_identity::no_links(root).map_err(|_| error(Code::Integrity))?;
    if read_small(&root.join(OWNER), 64)? != catalog.inventory_sha256.as_bytes() {
        return Err(error(Code::Integrity));
    }
    let entries = catalog.entries(&read_small(&root.join("inventory.json"), 16 << 20)?)?;
    let expected: HashSet<_> = entries.iter().map(|entry| entry.path.as_str()).collect();
    for (i, item) in walkdir::WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .enumerate()
    {
        check()?;
        if i > 100_000 {
            return Err(error(Code::Limit));
        }
        let item = item.map_err(|_| error(Code::Integrity))?;
        if !not_link(&item.metadata().map_err(|_| error(Code::Integrity))?) {
            return Err(error(Code::Integrity));
        }
        if item.file_type().is_file() {
            let name = item
                .path()
                .strip_prefix(root)
                .map_err(|_| error(Code::Integrity))?
                .to_str()
                .ok_or_else(|| error(Code::Integrity))?
                .replace('\\', "/");
            if name != OWNER && name != "inventory.json" && !expected.contains(name.as_str()) {
                return Err(error(Code::Integrity));
            }
        } else if !item.file_type().is_dir() {
            return Err(error(Code::Integrity));
        }
    }
    let mut parents = HashSet::new();
    for (i, entry) in entries.iter().enumerate() {
        check()?;
        let path = root.join(relative(&entry.path)?);
        if parents.insert(
            path.parent()
                .ok_or_else(|| error(Code::Integrity))?
                .to_path_buf(),
        ) {
            crate::path_identity::no_links(path.parent().unwrap())
                .map_err(|_| error(Code::Integrity))?;
        }
        let file = leaf(&path)?;
        if file.metadata().map_err(|_| error(Code::Integrity))?.len() != entry.bytes
            || read_hash(file, entry.bytes, check)? != (entry.bytes, entry.sha256.clone())
        {
            return Err(error(Code::Integrity));
        }
        progress(i + 1);
    }
    Ok(())
}
/// Cleanup is confined to one directly owned child of the canonical parent.
/// Never use this for arbitrary user-selected folders or unmarked old caches.
pub fn cleanup(parent: &Path, stage: &Path, marker: &str) {
    let safe = || -> Result<bool> {
        let parent = crate::path_identity::canonical(parent).map_err(|_| error(Code::Integrity))?;
        let resolved =
            crate::path_identity::canonical(stage).map_err(|_| error(Code::Integrity))?;
        Ok(resolved.parent() == Some(parent.as_path())
            && read_small(&resolved.join(OWNER), 64)? == marker.as_bytes())
    };
    if safe().unwrap_or(false) {
        let _ = fs::remove_dir_all(stage);
    }
}
fn promote(stage: &Path, installed: &Path, check: Check<'_>) -> Result<()> {
    // Windows readers (including file scanners) can briefly deny renaming a
    // complete tree. Retry only those fixed native codes, within a short bound;
    // never change ACLs, replace a destination or retry other errors.
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        check()?;
        if installed.exists() {
            return Err(error(Code::Integrity));
        }
        match fs::rename(stage, installed) {
            Ok(()) => return Ok(()),
            Err(error)
                if cfg!(windows)
                    && matches!(error.raw_os_error(), Some(5 | 32 | 33))
                    && Instant::now() < until =>
            {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(error) => return Err(io_error("promote")(error)),
        }
    }
}
pub fn install_archive(
    parent: &Path,
    archive: &Path,
    catalog: &Catalog,
    check: Check<'_>,
    progress: &dyn Fn(usize),
) -> Result<PathBuf> {
    catalog.validate()?;
    check()?;
    let mut file = regular(archive)?;
    if file.metadata().map_err(|_| error(Code::Integrity))?.len() != catalog.archive.bytes
        || read_hash(&mut file, catalog.archive.bytes, check)?
            != (catalog.archive.bytes, catalog.archive.sha256.clone())
    {
        return Err(error(Code::Integrity));
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|_| error(Code::Integrity))?;
    mkdir(parent)?;
    let parent = crate::path_identity::canonical(parent).map_err(|_| error(Code::Integrity))?;
    let installed = parent.join(catalog.directory());
    if installed.exists() {
        verify(&installed, catalog, check, progress)?;
        return Ok(installed);
    }
    let mut nonce = [0u8; 16];
    getrandom::fill(&mut nonce).map_err(|_| error(Code::Failed))?;
    let stage = parent.join(format!(".stage-{}", digest(&nonce)));
    fs::create_dir(&stage).map_err(io_error("createStage"))?;
    fs::write(stage.join(OWNER), &catalog.inventory_sha256).map_err(io_error("writeOwner"))?;
    let result = unpack(&stage, file, catalog, check, progress).and_then(|()| {
        verify(&stage, catalog, check, &|_| {})?;
        check()?;
        promote(&stage, &installed, check)?;
        Ok(installed)
    });
    if result
        .as_ref()
        .is_err_and(|error| error.stage != Some("promote"))
    {
        cleanup(&parent, &stage, &catalog.inventory_sha256);
    }
    result
}
fn unpack(
    root: &Path,
    file: File,
    catalog: &Catalog,
    check: Check<'_>,
    progress: &dyn Fn(usize),
) -> Result<()> {
    let mut archive = zip::ZipArchive::new(file).map_err(|_| error(Code::Integrity))?;
    if archive.len() != catalog.file_count + 1 {
        return Err(error(Code::Integrity));
    }
    let mut raw = Vec::new();
    archive
        .by_name("inventory.json")
        .map_err(|_| error(Code::Integrity))?
        .take((16 << 20) + 1)
        .read_to_end(&mut raw)
        .map_err(|_| error(Code::Integrity))?;
    let entries = catalog.entries(&raw)?;
    let mut seen = HashSet::new();
    let mut parents = HashSet::new();
    for i in 0..archive.len() {
        check()?;
        let mut item = archive.by_index(i).map_err(|_| error(Code::Integrity))?;
        let name = item
            .name()
            .map_err(|_| error(Code::Integrity))?
            .into_owned();
        relative(&name)?;
        if item.is_dir()
            || item
                .unix_mode()
                .is_some_and(|mode| mode & 0o170000 != 0o100000)
            || !seen.insert(name.to_ascii_lowercase())
        {
            return Err(error(Code::Integrity));
        }
        if name == "inventory.json" {
            continue;
        }
        let entry = entries
            .binary_search_by(|entry| entry.path.cmp(&name))
            .ok()
            .map(|index| &entries[index])
            .ok_or_else(|| error(Code::Integrity))?;
        if item.size() != entry.bytes {
            return Err(error(Code::Integrity));
        }
        let path = root.join(relative(&name)?);
        let parent = path.parent().ok_or_else(|| error(Code::Integrity))?;
        if parents.insert(parent.to_path_buf()) {
            mkdir(parent)?;
        }
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(io_error("createMember"))?;
        let mut hash = Sha256::new();
        let mut count = 0u64;
        let mut buffer = [0; 64 << 10];
        loop {
            check()?;
            let n = item.read(&mut buffer).map_err(|_| error(Code::Integrity))?;
            if n == 0 {
                break;
            }
            count += n as u64;
            if count > entry.bytes {
                return Err(error(Code::Limit));
            }
            output
                .write_all(&buffer[..n])
                .map_err(io_error("writeMember"))?;
            hash.update(&buffer[..n]);
        }
        if count != entry.bytes || format!("{:x}", hash.finalize()) != entry.sha256 {
            return Err(error(Code::Integrity));
        }
        progress(seen.len());
    }
    fs::write(root.join("inventory.json"), raw).map_err(io_error("writeInventory"))?;
    Ok(())
}

#[cfg(test)]
#[path = "office_asset_tests.rs"]
mod tests;

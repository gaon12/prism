//! Install verified wheels as data, without pip, setup.py, scripts or shells.
//! The compiled inventory digest authenticates every resulting runtime file.
use crate::{
    ocr::{error, Code, Result},
    ocr_assets::{self as assets, Catalog, Entry, Kind},
};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

pub type Check<'a> = &'a dyn Fn() -> Result<()>;
fn not_link(meta: &fs::Metadata) -> bool {
    if meta.file_type().is_symlink() {
        return false;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
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
    let file = options.open(path).map_err(|_| error(Code::Failed))?;
    let metadata = file.metadata().map_err(|_| error(Code::Failed))?;
    if !metadata.is_file() || !not_link(&metadata) {
        return Err(error(Code::Failed));
    }
    Ok(file)
}
pub fn regular(path: &Path) -> Result<File> {
    crate::path_identity::no_links(path).map_err(|_| error(Code::Failed))?;
    leaf(path)
}
pub fn hash(mut reader: impl Read, limit: u64, check: Check<'_>) -> Result<(u64, String)> {
    let mut bytes = 0u64;
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 << 10];
    loop {
        check()?;
        let n = reader.read(&mut buffer).map_err(|_| error(Code::Failed))?;
        if n == 0 {
            break;
        }
        bytes = bytes
            .checked_add(n as u64)
            .ok_or_else(|| error(Code::LimitExceeded))?;
        if bytes > limit {
            return Err(error(Code::LimitExceeded));
        }
        hash.update(&buffer[..n]);
    }
    Ok((bytes, format!("{:x}", hash.finalize())))
}
pub fn member(kind: Kind, value: &str) -> Result<String> {
    assets::relative(value)?;
    if kind == Kind::Python {
        return Ok(format!("python/{value}"));
    }
    if kind != Kind::Wheel {
        return Err(error(Code::InvalidInput));
    }
    let parts: Vec<_> = value.split('/').collect();
    if parts[0].ends_with(".data") {
        if parts.len() < 3 {
            return Err(error(Code::InvalidInput));
        }
        let base = match parts[1] {
            "purelib" | "platlib" => "python/Lib/site-packages",
            "data" => "python",
            "headers" => "python/include",
            "scripts" => "python/Scripts",
            _ => return Err(error(Code::InvalidInput)),
        };
        Ok(format!("{base}/{}", parts[2..].join("/")))
    } else {
        Ok(format!("python/Lib/site-packages/{value}"))
    }
}
pub fn write(root: &Path, name: &str, bytes: &[u8], entries: &mut Vec<Entry>) -> Result<()> {
    let path = root.join(assets::relative(name)?);
    fs::create_dir_all(path.parent().ok_or_else(|| error(Code::Failed))?)
        .map_err(|_| error(Code::Failed))?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| error(Code::Failed))?;
    file.write_all(bytes).map_err(|_| error(Code::Failed))?;
    file.sync_all().map_err(|_| error(Code::Failed))?;
    entries.push(Entry {
        path: name.into(),
        bytes: bytes.len() as u64,
        sha256: assets::digest(bytes),
    });
    Ok(())
}
pub fn unpack(
    path: &Path,
    kind: Kind,
    catalog: &Catalog,
    root: &Path,
    entries: &mut Vec<Entry>,
    check: Check<'_>,
    progress: &dyn Fn(usize),
) -> Result<()> {
    let mut archive = zip::ZipArchive::new(regular(path)?).map_err(|_| error(Code::Failed))?;
    if archive.len() > 60_000 {
        return Err(error(Code::LimitExceeded));
    }
    let mut total: u64 = entries.iter().map(|entry| entry.bytes).sum();
    let mut parents = std::collections::HashSet::new();
    for i in 0..archive.len() {
        check()?;
        let mut item = archive.by_index(i).map_err(|_| error(Code::Failed))?;
        if item.is_dir() {
            continue;
        }
        let name = item.name().map_err(|_| error(Code::Failed))?.into_owned();
        if item
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err(error(Code::Failed));
        }
        if kind == Kind::Python && name.ends_with("._pth") {
            continue;
        }
        let name = member(kind, &name)?;
        let length = item.size();
        total = total
            .checked_add(length)
            .ok_or_else(|| error(Code::LimitExceeded))?;
        if entries.len() >= catalog.file_count || total > catalog.expanded_bytes {
            return Err(error(Code::LimitExceeded));
        }
        let destination = root.join(assets::relative(&name)?);
        let parent = destination.parent().ok_or_else(|| error(Code::Failed))?;
        fs::create_dir_all(parent).map_err(|_| error(Code::Failed))?;
        if parents.insert(parent.to_path_buf()) {
            crate::path_identity::no_links(parent).map_err(|_| error(Code::Failed))?;
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&destination)
            .map_err(|_| error(Code::Failed))?;
        let mut size = 0u64;
        let mut hash = Sha256::new();
        let mut buffer = [0; 64 << 10];
        loop {
            check()?;
            let n = item.read(&mut buffer).map_err(|_| error(Code::Failed))?;
            if n == 0 {
                break;
            }
            size += n as u64;
            if size > length {
                return Err(error(Code::LimitExceeded));
            }
            file.write_all(&buffer[..n])
                .map_err(|_| error(Code::Failed))?;
            hash.update(&buffer[..n]);
        }
        if size != length {
            return Err(error(Code::Failed));
        }
        // Files are private staged data until the full inventory and directory
        // commit. Per-file fsync for 34k immutable wheel files is unnecessary;
        // a damaged/interrupted install is never eligible to start a process.
        entries.push(Entry {
            path: name,
            bytes: size,
            sha256: format!("{:x}", hash.finalize()),
        });
        progress(entries.len());
    }
    Ok(())
}
pub fn load(root: &Path, catalog: &Catalog) -> Result<Vec<Entry>> {
    let file = regular(&root.join("inventory.json"))?;
    if file.metadata().map_err(|_| error(Code::Failed))?.len() > 16 << 20 {
        return Err(error(Code::LimitExceeded));
    }
    let mut bytes = Vec::new();
    file.take((16 << 20) + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| error(Code::Failed))?;
    if bytes.len() > 16 << 20 {
        return Err(error(Code::LimitExceeded));
    }
    let entries: Vec<Entry> = serde_json::from_slice(&bytes).map_err(|_| error(Code::Failed))?;
    catalog.validate_inventory(&entries)?;
    Ok(entries)
}
pub fn verify(root: &Path, catalog: &Catalog, entries: &[Entry], check: Check<'_>) -> Result<()> {
    catalog.validate_inventory(entries)?;
    crate::path_identity::no_links(root).map_err(|_| error(Code::Failed))?;
    let expected: std::collections::HashSet<_> =
        entries.iter().map(|entry| entry.path.as_str()).collect();
    // An extra module or bytecode cache can affect Python imports even when all
    // original .py files have correct hashes. Only the separate cache directory
    // may contain generated files; the interpreter runs with -I and -B.
    let walk = walkdir::WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            !(entry.depth() == 1
                && entry.file_name() == "cache"
                && entry.file_type().is_dir()
                && crate::path_identity::no_links(entry.path()).is_ok())
        });
    for (i, entry) in walk.enumerate() {
        check()?;
        if i > 100_000 {
            return Err(error(Code::LimitExceeded));
        }
        let entry = entry.map_err(|_| error(Code::Failed))?;
        if !not_link(&entry.metadata().map_err(|_| error(Code::Failed))?) {
            return Err(error(Code::Failed));
        }
        if entry.file_type().is_file() {
            let relative = entry
                .path()
                .strip_prefix(root)
                .map_err(|_| error(Code::Failed))?
                .to_str()
                .ok_or_else(|| error(Code::Failed))?
                .replace('\\', "/");
            if relative != "inventory.json" && !expected.contains(relative.as_str()) {
                return Err(error(Code::Failed));
            }
        } else if !entry.file_type().is_dir() {
            return Err(error(Code::Failed));
        }
    }
    let mut parents = std::collections::HashSet::new();
    for entry in entries {
        check()?;
        let path = root.join(assets::relative(&entry.path)?);
        let parent = path.parent().ok_or_else(|| error(Code::Failed))?;
        // Recheck each directory once while opening every actual leaf without
        // following it. Every file still has its handle bytes hash-verified.
        if parents.insert(parent.to_path_buf()) {
            crate::path_identity::no_links(parent).map_err(|_| error(Code::Failed))?;
        }
        let file = leaf(&path)?;
        if file.metadata().map_err(|_| error(Code::Failed))?.len() != entry.bytes
            || hash(file, entry.bytes, check)? != (entry.bytes, entry.sha256.clone())
        {
            return Err(error(Code::Failed));
        }
    }
    crate::path_identity::no_links(root).map_err(|_| error(Code::Failed))?;
    Ok(())
}
/// Remove only an exact stage this operation created below the canonical OCR
/// directory. Never recursively delete a caller-provided or installed path.
pub fn cleanup(parent: &Path, stage: &Path) {
    if stage.parent() != Some(parent)
        || !stage
            .file_name()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.starts_with("stage-") && s.len() == 38)
    {
        return;
    }
    if crate::path_identity::no_links(stage).is_ok() {
        let _ = fs::remove_dir_all(stage);
    }
}
pub fn installed(root: &Path, catalog: &Catalog) -> PathBuf {
    root.join(catalog.directory())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wheel_installation_uses_data_schemes_without_running_scripts() {
        assert_eq!(
            member(Kind::Wheel, "module/a.py").unwrap(),
            "python/Lib/site-packages/module/a.py"
        );
        assert_eq!(
            member(Kind::Wheel, "x.data/platlib/a.dll").unwrap(),
            "python/Lib/site-packages/a.dll"
        );
        assert_eq!(
            member(Kind::Wheel, "x.data/data/share/notice").unwrap(),
            "python/share/notice"
        );
        assert!(member(Kind::Wheel, "x.data/unknown/a").is_err());
        assert!(member(Kind::Wheel, "../escape").is_err());
    }
    #[test]
    fn cancelled_or_unsafe_archives_never_escape_the_owned_stage() {
        use zip::write::SimpleFileOptions;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("stage");
        fs::create_dir(&root).unwrap();
        let archive = dir.path().join("bad.whl");
        let mut zip = zip::ZipWriter::new(File::create(&archive).unwrap());
        zip.start_file("../escape.py", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"bad").unwrap();
        zip.finish().unwrap();
        let catalog = assets::for_target("windows-x64").unwrap();
        let mut files = Vec::new();
        assert!(unpack(
            &archive,
            Kind::Wheel,
            &catalog,
            &root,
            &mut files,
            &|| Ok(()),
            &|_| {}
        )
        .is_err());
        assert!(files.is_empty() && !dir.path().join("escape.py").exists());
        assert_eq!(
            unpack(
                &archive,
                Kind::Wheel,
                &catalog,
                &root,
                &mut files,
                &|| Err(error(Code::Cancelled)),
                &|_| {}
            )
            .unwrap_err()
            .code,
            Code::Cancelled
        );
    }
}

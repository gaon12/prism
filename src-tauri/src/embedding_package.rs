//! Read only the layouts of the pinned upstream runtime packages. Unix library
//! symlinks become checked regular copies: extraction never creates filesystem links.
use crate::embedding_assets::{ArchiveFormat, RuntimePackage};
use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

const MAX_ENTRIES: usize = 128;
const MAX_EXPANDED: u64 = 400 * 1024 * 1024;
type Result<T> = std::result::Result<T, String>;
type Cancel<'a> = &'a dyn Fn() -> Result<()>;
fn invalid() -> String {
    "Unsafe or damaged CPU runtime archive".into()
}
fn name(value: &str) -> Result<String> {
    let stem = value.split('.').next().unwrap_or("").to_ascii_uppercase();
    if value.is_empty()
        || value.len() > 255
        || matches!(value, "." | "..")
        || value.ends_with(['.', ' '])
        || value
            .chars()
            .any(|c| c.is_control() || "/\\:<>\"|?*".contains(c))
        || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"].iter().any(|p| {
            stem.strip_prefix(p)
                .is_some_and(|s| matches!(s, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9"))
        })
    {
        return Err(invalid());
    }
    Ok(value.into())
}
fn tar_name(value: &str) -> Result<String> {
    name(value.strip_prefix("llama-b11960/").ok_or_else(invalid)?)
}
fn not_link(path: &Path) -> Result<fs::Metadata> {
    let meta = fs::symlink_metadata(path).map_err(|_| invalid())?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return Err(invalid());
        }
    }
    if meta.file_type().is_symlink() {
        return Err(invalid());
    }
    Ok(meta)
}
fn root(path: &Path) -> Result<PathBuf> {
    if !path.is_absolute() || !not_link(path)?.is_dir() {
        return Err(invalid());
    }
    fs::canonicalize(path).map_err(|_| invalid())
}
fn hash(mut reader: impl Read, cancel: Cancel<'_>) -> Result<(u64, String)> {
    let mut hash = Sha256::new();
    let mut bytes = 0u64;
    let mut buffer = [0u8; 65536];
    loop {
        cancel()?;
        let count = reader.read(&mut buffer).map_err(|_| invalid())?;
        if count == 0 {
            break;
        }
        bytes += count as u64;
        if bytes > MAX_EXPANDED {
            return Err(invalid());
        }
        hash.update(&buffer[..count]);
    }
    Ok((bytes, format!("{:x}", hash.finalize())))
}
#[derive(Clone)]
struct Entry {
    size: u64,
    hash: String,
    executable: bool,
}
struct Plan<'a> {
    destination: PathBuf,
    extracting: bool,
    seen: HashSet<String>,
    files: BTreeMap<String, Entry>,
    aliases: BTreeMap<String, String>,
    expanded: u64,
    cancel: Cancel<'a>,
}
impl Plan<'_> {
    fn reserve(&mut self, value: &str) -> Result<()> {
        if self.seen.len() >= MAX_ENTRIES || !self.seen.insert(value.to_lowercase()) {
            return Err(invalid());
        }
        Ok(())
    }
    fn regular(
        &mut self,
        value: String,
        size: u64,
        executable: bool,
        mut reader: impl Read,
    ) -> Result<()> {
        self.reserve(&value)?;
        self.expanded = self
            .expanded
            .checked_add(size)
            .filter(|n| *n <= MAX_EXPANDED)
            .ok_or_else(invalid)?;
        let path = self.destination.join(&value);
        let (actual, digest) = if self.extracting {
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .map_err(|_| invalid())?;
            let mut hasher = Sha256::new();
            let mut copied = 0u64;
            let mut buffer = [0u8; 65536];
            loop {
                (self.cancel)()?;
                let count = reader.read(&mut buffer).map_err(|_| invalid())?;
                if count == 0 {
                    break;
                }
                copied += count as u64;
                if copied > size {
                    return Err(invalid());
                }
                hasher.update(&buffer[..count]);
                output.write_all(&buffer[..count]).map_err(|_| invalid())?;
            }
            output.sync_all().map_err(|_| invalid())?;
            permissions(&path, executable)?;
            (copied, format!("{:x}", hasher.finalize()))
        } else {
            hash(reader, self.cancel)?
        };
        if actual != size {
            return Err(invalid());
        }
        self.files.insert(
            value,
            Entry {
                size,
                hash: digest,
                executable,
            },
        );
        Ok(())
    }
    fn alias(&mut self, value: String, target: String) -> Result<()> {
        self.reserve(&value)?;
        self.aliases.insert(value, target);
        Ok(())
    }
    fn finish(mut self, package: RuntimePackage) -> Result<()> {
        // Resolve only links within the same flat package, with bounded chains.
        // A link cycle or missing target fails before any alias is materialized.
        let mut resolved = Vec::new();
        for (alias, target) in &self.aliases {
            let mut target = target;
            let mut visited = HashSet::new();
            while let Some(next) = self.aliases.get(target) {
                if !visited.insert(target) {
                    return Err(invalid());
                }
                target = next;
            }
            let entry = self.files.get(target).ok_or_else(invalid)?.clone();
            self.expanded = self
                .expanded
                .checked_add(entry.size)
                .filter(|n| *n <= MAX_EXPANDED)
                .ok_or_else(invalid)?;
            resolved.push((alias.clone(), target.clone(), entry));
        }
        for (alias, target, entry) in resolved {
            (self.cancel)()?;
            if self.extracting {
                let mut input = File::open(self.destination.join(target)).map_err(|_| invalid())?;
                let path = self.destination.join(&alias);
                let mut output = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                    .map_err(|_| invalid())?;
                if std::io::copy(&mut input, &mut output).map_err(|_| invalid())? != entry.size {
                    return Err(invalid());
                }
                output.sync_all().map_err(|_| invalid())?;
                permissions(&path, entry.executable)?;
            }
            self.files.insert(alias, entry);
        }
        if !self
            .files
            .get(package.executable)
            .is_some_and(|e| e.executable)
        {
            return Err("CPU runtime archive has no executable server".into());
        }
        // Refuse extra binaries or links added after extraction. Do not let dynamic
        // library discovery load an unverified file from the runtime directory.
        let actual: HashSet<String> = fs::read_dir(&self.destination)
            .map_err(|_| invalid())?
            .map(|entry| {
                entry
                    .map_err(|_| invalid())
                    .and_then(|entry| entry.file_name().into_string().map_err(|_| invalid()))
            })
            .collect::<Result<_>>()?;
        if actual != self.files.keys().cloned().collect::<HashSet<_>>() {
            return Err("CPU runtime directory contains unexpected files".into());
        }
        for (value, entry) in self.files {
            (self.cancel)()?;
            let path = self.destination.join(value);
            let metadata = not_link(&path)?;
            if !metadata.is_file() || metadata.len() != entry.size {
                return Err(invalid());
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if entry.executable && metadata.permissions().mode() & 0o111 == 0 {
                    return Err("CPU runtime file is not executable".into());
                }
            }
            if hash(File::open(path).map_err(|_| invalid())?, self.cancel)?.1 != entry.hash {
                return Err("CPU runtime SHA-256 mismatch; import or download it again".into());
            }
        }
        Ok(())
    }
}
fn permissions(path: &Path, executable: bool) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            path,
            fs::Permissions::from_mode(if executable { 0o700 } else { 0o600 }),
        )
        .map_err(|_| invalid())?;
    }
    let _ = (path, executable);
    Ok(())
}

pub fn inspect(
    archive: &Path,
    destination: &Path,
    package: RuntimePackage,
    extracting: bool,
    cancel: Cancel<'_>,
) -> Result<()> {
    let destination = root(destination)?;
    let mut plan = Plan {
        destination,
        extracting,
        seen: HashSet::new(),
        files: BTreeMap::new(),
        aliases: BTreeMap::new(),
        expanded: 0,
        cancel,
    };
    let source = File::open(archive).map_err(|_| "Cannot open CPU runtime archive".to_string())?;
    match package.format {
        ArchiveFormat::Zip => {
            let mut archive = zip::ZipArchive::new(source).map_err(|_| invalid())?;
            if archive.len() > MAX_ENTRIES {
                return Err(invalid());
            }
            for index in 0..archive.len() {
                cancel()?;
                let mut entry = archive.by_index(index).map_err(|_| invalid())?;
                if !entry.is_file() || entry.is_symlink() {
                    return Err(invalid());
                }
                let value = name(&entry.name().map_err(|_| invalid())?)?;
                let size = entry.size();
                let executable = value == package.executable || value.ends_with(".exe");
                plan.regular(value, size, executable, &mut entry)?;
            }
        }
        ArchiveFormat::TarGz => {
            let mut archive = tar::Archive::new(GzDecoder::new(source));
            let mut count = 0;
            for entry in archive.entries().map_err(|_| invalid())? {
                cancel()?;
                count += 1;
                if count > MAX_ENTRIES + 1 {
                    return Err(invalid());
                }
                let mut entry = entry.map_err(|_| invalid())?;
                let path = std::str::from_utf8(&entry.path_bytes())
                    .map_err(|_| invalid())?
                    .to_owned();
                let kind = entry.header().entry_type();
                if kind.is_dir() && path.trim_end_matches('/') == "llama-b11960" {
                    continue;
                }
                let value = tar_name(&path)?;
                if kind.is_file() {
                    let executable = entry.header().mode().map_err(|_| invalid())? & 0o111 != 0;
                    plan.regular(value, entry.size(), executable, &mut entry)?;
                } else if kind.is_symlink() || kind.is_hard_link() {
                    let target = entry
                        .link_name()
                        .map_err(|_| invalid())?
                        .ok_or_else(invalid)?;
                    let target = target.to_str().ok_or_else(invalid)?;
                    let target = if kind.is_hard_link() {
                        tar_name(target)?
                    } else {
                        name(target)?
                    };
                    plan.alias(value, target)?;
                } else {
                    return Err(invalid());
                }
            }
        }
    }
    plan.finish(package)
}

#[cfg(test)]
#[path = "embedding_package_tests.rs"]
mod tests;

//! Read the pinned relocatable Unix interpreter as data. Internal archive
//! aliases become verified regular copies; no filesystem links are installed.
use super::{assets, error, leaf, regular, Catalog, Check, Code, Entry, Result};
use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::Path,
};

fn name(value: &str) -> Result<String> {
    assets::relative(value)?;
    if !value.starts_with("python/") {
        return Err(error(Code::Failed));
    }
    Ok(value.into())
}

fn target(alias: &str, value: &str, hard: bool) -> Result<String> {
    if value.is_empty()
        || value.starts_with('/')
        || value.contains(['\\', ':'])
        || value.chars().any(char::is_control)
    {
        return Err(error(Code::Failed));
    }
    let mut parts: Vec<_> = if hard {
        vec![]
    } else {
        alias.split('/').collect()
    };
    if !hard {
        parts.pop();
    }
    for part in value.split('/') {
        match part {
            "" | "." => {}
            ".." if parts.len() > 1 => {
                parts.pop();
            }
            ".." => return Err(error(Code::Failed)),
            _ => parts.push(part),
        }
    }
    name(&parts.join("/"))
}

struct Installer<'a> {
    root: &'a Path,
    catalog: &'a Catalog,
    entries: &'a mut Vec<Entry>,
    total: u64,
    parents: HashSet<std::path::PathBuf>,
    check: Check<'a>,
    progress: &'a dyn Fn(usize),
}
impl Installer<'_> {
    fn copy(&mut self, name: String, size: u64, mut source: impl Read) -> Result<Entry> {
        (self.check)()?;
        self.total = self
            .total
            .checked_add(size)
            .filter(|total| *total <= self.catalog.expanded_bytes)
            .ok_or_else(|| error(Code::LimitExceeded))?;
        if self.entries.len() >= self.catalog.file_count {
            return Err(error(Code::LimitExceeded));
        }
        let destination = self.root.join(assets::relative(&name)?);
        let parent = destination.parent().ok_or_else(|| error(Code::Failed))?;
        fs::create_dir_all(parent).map_err(|_| error(Code::Failed))?;
        if self.parents.insert(parent.to_path_buf()) {
            crate::path_identity::no_links(parent).map_err(|_| error(Code::Failed))?;
        }
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&destination)
            .map_err(|_| error(Code::Failed))?;
        let mut copied = 0u64;
        let mut sha = Sha256::new();
        let mut buffer = [0u8; 64 << 10];
        loop {
            (self.check)()?;
            let count = source.read(&mut buffer).map_err(|_| error(Code::Failed))?;
            if count == 0 {
                break;
            }
            copied += count as u64;
            if copied > size {
                return Err(error(Code::LimitExceeded));
            }
            sha.update(&buffer[..count]);
            output
                .write_all(&buffer[..count])
                .map_err(|_| error(Code::Failed))?;
        }
        if copied != size {
            return Err(error(Code::Failed));
        }
        // Only the pinned interpreter is launched. Preserve the user's umask
        // and grant owner-execute; never install setuid or archive permission bits.
        #[cfg(unix)]
        if name == self.catalog.executable {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = output
                .metadata()
                .map_err(|_| error(Code::Failed))?
                .permissions();
            permissions.set_mode((permissions.mode() & 0o777) | 0o100);
            output
                .set_permissions(permissions)
                .map_err(|_| error(Code::Failed))?;
        }
        let entry = Entry {
            path: name,
            bytes: size,
            sha256: format!("{:x}", sha.finalize()),
        };
        self.entries.push(entry.clone());
        (self.progress)(self.entries.len());
        Ok(entry)
    }
}

pub(super) fn unpack(
    archive: &Path,
    catalog: &Catalog,
    root: &Path,
    entries: &mut Vec<Entry>,
    check: Check<'_>,
    progress: &dyn Fn(usize),
) -> Result<()> {
    let total = entries.iter().map(|entry| entry.bytes).sum();
    let mut reserved: HashSet<_> = entries.iter().map(|entry| entry.path.clone()).collect();
    let mut installer = Installer {
        root,
        catalog,
        entries,
        total,
        parents: HashSet::new(),
        check,
        progress,
    };
    let mut archive = tar::Archive::new(GzDecoder::new(regular(archive)?));
    let mut files = BTreeMap::new();
    let mut aliases = BTreeMap::new();
    for (count, entry) in archive
        .entries()
        .map_err(|_| error(Code::Failed))?
        .enumerate()
    {
        check()?;
        if count > 100_000 {
            return Err(error(Code::LimitExceeded));
        }
        let mut entry = entry.map_err(|_| error(Code::Failed))?;
        let raw = entry.path_bytes();
        let value = std::str::from_utf8(&raw).map_err(|_| error(Code::Failed))?;
        let kind = entry.header().entry_type();
        if kind.is_dir() {
            let value = value.trim_end_matches('/');
            if value != "python" {
                name(value)?;
            }
            continue;
        }
        let value = name(value)?;
        if !reserved.insert(value.clone()) {
            return Err(error(Code::Failed));
        }
        if reserved.len() > catalog.file_count {
            return Err(error(Code::LimitExceeded));
        }
        if kind.is_file() {
            let size = entry.size();
            files.insert(value.clone(), installer.copy(value, size, &mut entry)?);
        } else if kind.is_symlink() || kind.is_hard_link() {
            let link = entry.link_name_bytes().ok_or_else(|| error(Code::Failed))?;
            let link = std::str::from_utf8(&link).map_err(|_| error(Code::Failed))?;
            aliases.insert(value.clone(), target(&value, link, kind.is_hard_link())?);
        } else {
            return Err(error(Code::Failed));
        }
    }
    // Validate every chain before writing any alias. References must end at a
    // regular member of this archive, not a preexisting stage or host file.
    let mut resolved = Vec::new();
    for (alias, target) in &aliases {
        check()?;
        let mut target = target;
        let mut visited = HashSet::from([alias]);
        while let Some(next) = aliases.get(target) {
            if !visited.insert(target) {
                return Err(error(Code::Failed));
            }
            target = next;
        }
        let entry = files.get(target).ok_or_else(|| error(Code::Failed))?;
        resolved.push((alias.clone(), target.clone(), entry.clone()));
    }
    for (alias, source, expected) in resolved {
        let file = regular(&root.join(source))?;
        let actual = installer.copy(alias, expected.bytes, file)?;
        if actual.sha256 != expected.sha256 {
            return Err(error(Code::Failed));
        }
    }
    // The main interpreter must be an actual regular file even if upstream
    // used an alias for its public binary name.
    leaf(&root.join(assets::relative(&catalog.executable)?))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{write::GzEncoder, Compression};

    fn fixture(path: &Path, aliases: &[(&str, &str)], regular: bool) {
        let mut archive = tar::Builder::new(GzEncoder::new(
            std::fs::File::create(path).unwrap(),
            Compression::fast(),
        ));
        for (name, target) in aliases {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_size(0);
            header.set_mode(0o777);
            header.set_link_name(target).unwrap();
            header.set_cksum();
            archive
                .append_data(&mut header, name, std::io::empty())
                .unwrap();
        }
        if regular {
            let mut header = tar::Header::new_gnu();
            header.set_size(3);
            header.set_mode(0o6755);
            header.set_cksum();
            archive
                .append_data(&mut header, "python/bin/python3.13", &b"abc"[..])
                .unwrap();
        }
        archive.into_inner().unwrap().finish().unwrap();
    }

    #[test]
    fn forward_aliases_are_regular_copies_and_do_not_install_archive_privileges() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("python.tar.gz");
        fixture(
            &archive,
            &[
                ("python/bin/python", "python3"),
                ("python/bin/python3", "python3.13"),
            ],
            true,
        );
        let root = dir.path().join("stage");
        fs::create_dir(&root).unwrap();
        let catalog = assets::for_target("linux-x64").unwrap();
        let mut entries = vec![];
        unpack(&archive, &catalog, &root, &mut entries, &|| Ok(()), &|_| {}).unwrap();
        assert_eq!(entries.len(), 3);
        for entry in entries {
            let path = root.join(entry.path);
            assert!(fs::symlink_metadata(&path).unwrap().is_file());
            assert_eq!(fs::read(path).unwrap(), b"abc");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(root.join(&catalog.executable))
                .unwrap()
                .permissions()
                .mode();
            assert_ne!(mode & 0o100, 0);
            assert_eq!(mode & 0o6000, 0);
        }
    }

    #[test]
    fn cyclic_external_missing_and_expanding_aliases_fail_inside_the_stage() {
        for (aliases, budget) in [
            (vec![("python/bin/a", "b"), ("python/bin/b", "a")], None),
            (vec![("python/bin/a", "../../../outside")], None),
            (vec![("python/bin/a", "/outside")], None),
            (vec![("python/bin/a", "missing")], None),
            (
                vec![("python/bin/a", "python3.13"), ("python/bin/b", "a")],
                Some(6),
            ),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let archive = dir.path().join("python.tar.gz");
            fixture(&archive, &aliases, true);
            let root = dir.path().join("stage");
            fs::create_dir(&root).unwrap();
            let mut catalog = assets::for_target("linux-x64").unwrap();
            if let Some(budget) = budget {
                catalog.expanded_bytes = budget;
            }
            assert!(unpack(&archive, &catalog, &root, &mut vec![], &|| Ok(()), &|_| {}).is_err());
            assert!(!dir.path().join("outside").exists());
        }
        assert_eq!(
            target("python/share/1/alias", "../a/real", false).unwrap(),
            "python/share/a/real"
        );
        assert_eq!(
            target("python/bin/alias", "python/bin/real", true).unwrap(),
            "python/bin/real"
        );
    }
}

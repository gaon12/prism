//! Snapshot-scoped, read-only facts for inclusion policies. Callers still own
//! scope authorization. Recreate for each scan/query/revalidation so cached
//! ancestor attributes and ignore files do not become an access authority.
use crate::file_policy::{Categories, Category, Decision, Facts};
use std::{
    collections::HashMap,
    fs, io,
    path::{Component, Path, PathBuf},
};

#[path = "file_policy_git.rs"]
mod git;

const MAX_DEPTH: usize = 256;
const MAX_DIRECTORIES: usize = 4096;
const MAX_DIRECTORY_KEY_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy)]
struct Directory {
    hidden: bool,
    system: bool,
    git: Option<git::Marker>,
}

pub struct Reader {
    policy: Categories,
    root: PathBuf,
    repository_start: Option<PathBuf>,
    root_checked: bool,
    directories: HashMap<PathBuf, Directory>,
    directory_key_bytes: usize,
    rules: git::Rules,
    system_roots: Option<Vec<PathBuf>>,
}

impl Reader {
    pub fn new(policy: Categories, root: &Path) -> Self {
        Self {
            policy,
            root: root.to_owned(),
            repository_start: None,
            root_checked: false,
            directories: HashMap::new(),
            directory_key_bytes: 0,
            rules: git::Rules::default(),
            system_roots: None,
        }
    }

    /// Resolve only facts required by exclusions. The unrestricted case does
    /// not inspect the filesystem, including for an unavailable path.
    pub fn check(&mut self, path: &Path, is_dir: bool) -> io::Result<Decision> {
        self.check_inner(path, is_dir, None)
    }

    /// Filter a trusted Windows filename snapshot without statting each leaf.
    /// The caller owns root/stream validation and supplies the kind and complete
    /// own attributes from the same record. None retains the fresh-stat fallback.
    /// Ancestors/rules are still checked. This is not an access/read authority:
    /// original-file reads and actions must use check() and fresh authorization.
    #[cfg(windows)]
    pub fn check_windows_snapshot(
        &mut self,
        path: &Path,
        is_dir: bool,
        attributes: Option<u32>,
    ) -> io::Result<Decision> {
        if attributes.is_some_and(|flags| flags & 0x400 != 0) {
            return Err(unsafe_path());
        }
        let known = if self.policy.needs_path_attributes() {
            attributes.map(|flags| windows_attributes(path, flags))
        } else {
            None
        };
        self.check_inner(path, is_dir, known)
    }

    fn check_inner(
        &mut self,
        path: &Path,
        is_dir: bool,
        known_own_attributes: Option<(bool, bool)>,
    ) -> io::Result<Decision> {
        if !path.is_absolute()
            || !self.root.is_absolute()
            || !within(path, &self.root)
            || path
                .components()
                .chain(self.root.components())
                .any(|c| matches!(c, Component::ParentDir))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Expected an absolute policy path without parent components.",
            ));
        }
        if self.policy.unrestricted() {
            return Ok(Decision::Include);
        }
        let mut facts = Facts::from_name(path, is_dir);
        if let Some((hidden, system)) = known_own_attributes {
            // Positive own flags can exclude immediately. Negative own flags
            // say nothing about ancestors, which still need their normal checks.
            if hidden {
                facts.hidden = Some(true);
            }
            if system {
                facts.system = Some(true);
            }
        }
        match self.policy.decide(&facts) {
            Decision::NeedsEvidence(_) => {}
            decision => return Ok(decision),
        }
        let mut parents: Vec<_> = path.ancestors().skip(1).collect();
        if parents.len() > MAX_DEPTH {
            return Err(budget());
        }
        parents.reverse();
        if !self.root_checked {
            crate::path_identity::no_links(&self.root)?;
            self.root_checked = true;
        }
        if !self.policy.repository_ignored && self.repository_start.is_none() {
            self.repository_start = Some(git::repository_start(&self.root)?);
        }
        let mut active = git::Active::default();
        for parent in parents {
            let in_scope = within(parent, &self.root);
            let rule_ancestor = !self.policy.repository_ignored
                && self
                    .repository_start
                    .as_ref()
                    .is_some_and(|start| within(parent, start));
            if !in_scope && !rule_ancestor {
                continue;
            }
            let directory = self.directory(parent)?;
            if in_scope && !self.policy.hidden && directory.hidden {
                return Ok(Decision::Exclude(Category::Hidden));
            }
            if in_scope && !self.policy.system && directory.system {
                return Ok(Decision::Exclude(Category::System));
            }
            if rule_ancestor {
                // An ignored parent cannot be reopened by a child's rule or
                // a nested repository, matching recursive Gitignore traversal.
                if active.ignored(parent, true) {
                    return Ok(Decision::Exclude(Category::RepositoryIgnored));
                }
                if let Some(marker) = directory.git {
                    active = self.rules.repository(parent, marker)?;
                }
                if active.in_repository() {
                    self.rules.add_directory(&mut active, parent)?;
                }
            }
        }
        if self.policy.needs_path_attributes() {
            let (hidden, system) = if let Some(known) = known_own_attributes {
                known
            } else {
                let metadata = plain_metadata(path)?;
                if metadata.is_dir() != is_dir {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "The policy path changed type.",
                    ));
                }
                attributes(path, &metadata)
            };
            facts.hidden = Some(hidden);
            facts.system = Some(system || self.system_path(path)?);
        }
        if !self.policy.repository_ignored {
            facts.repository_ignored = Some(active.ignored(path, is_dir));
        }
        match self.policy.decide(&facts) {
            Decision::NeedsEvidence(_) => Err(io::Error::other(
                "Required file policy evidence is unavailable.",
            )),
            decision => Ok(decision),
        }
    }

    fn directory(&mut self, path: &Path) -> io::Result<Directory> {
        if let Some(directory) = self.directories.get(path) {
            return Ok(*directory);
        }
        let metadata = plain_metadata(path)?;
        if !metadata.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                "A policy ancestor is not a directory.",
            ));
        }
        // Volume roots can carry HIDDEN/SYSTEM even though their ordinary
        // children are not hidden or system paths. Never hide a whole drive
        // merely because its root record carries those implementation flags.
        let (hidden, system) = if path.file_name().is_none() {
            (false, false)
        } else {
            attributes(path, &metadata)
        };
        let directory = Directory {
            hidden,
            system: system || self.system_path(path)?,
            git: if !self.policy.repository_ignored
                && self
                    .repository_start
                    .as_ref()
                    .is_some_and(|start| within(path, start))
            {
                git::marker(path)?
            } else {
                None
            },
        };
        let bytes = path.as_os_str().len();
        if self.directories.len() >= MAX_DIRECTORIES
            || self.directory_key_bytes.saturating_add(bytes) > MAX_DIRECTORY_KEY_BYTES
        {
            self.directories.clear();
            self.directory_key_bytes = 0;
        }
        self.directory_key_bytes += bytes;
        self.directories.insert(path.to_owned(), directory);
        Ok(directory)
    }

    fn system_path(&mut self, path: &Path) -> io::Result<bool> {
        if self.policy.system {
            return Ok(false);
        }
        if self.system_roots.is_none() {
            self.system_roots = Some(system_roots()?);
        }
        Ok(self
            .system_roots
            .as_ref()
            .unwrap()
            .iter()
            .any(|root| within(path, root)))
    }
}

fn within(path: &Path, root: &Path) -> bool {
    #[cfg(windows)]
    {
        if path.starts_with(root) {
            return true;
        }
        let path = crate::path_identity::normal(path)
            .to_string_lossy()
            .replace('/', "\\")
            .to_lowercase();
        let root = crate::path_identity::normal(root)
            .to_string_lossy()
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_lowercase();
        path == root
            || path
                .strip_prefix(&root)
                .is_some_and(|tail| tail.starts_with('\\'))
    }
    #[cfg(not(windows))]
    {
        path.starts_with(root)
    }
}

fn dot_hidden(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name.as_encoded_bytes().starts_with(b"."))
}

#[cfg(windows)]
fn windows_attributes(path: &Path, flags: u32) -> (bool, bool) {
    if path.file_name().is_none() {
        return (false, false);
    }
    (dot_hidden(path) || flags & 2 != 0, flags & 4 != 0)
}

fn attributes(path: &Path, metadata: &fs::Metadata) -> (bool, bool) {
    // The selected volume entry is also visited by filter_entry, not only as
    // an ancestor. Its implementation flags must never prune the whole drive.
    if path.file_name().is_none() {
        return (false, false);
    }
    #[cfg(not(windows))]
    let dot = dot_hidden(path);
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        windows_attributes(path, metadata.file_attributes())
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::darwin::fs::MetadataExt;
        (dot || metadata.st_flags() & libc::UF_HIDDEN != 0, false)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = metadata;
        (dot, false)
    }
}

fn plain_metadata(path: &Path) -> io::Result<fs::Metadata> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(unsafe_path());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(unsafe_path());
        }
    }
    Ok(metadata)
}

fn unsafe_path() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "Linked policy metadata is unsupported.",
    )
}
fn budget() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "File policy metadata exceeds its worker budget.",
    )
}

#[cfg(windows)]
fn system_roots() -> io::Result<Vec<PathBuf>> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::{
        System::Com::CoTaskMemFree,
        UI::Shell::{
            FOLDERID_ProgramData, FOLDERID_ProgramFiles, FOLDERID_ProgramFilesX86,
            FOLDERID_Windows, SHGetKnownFolderPath,
        },
    };
    let mut roots = Vec::new();
    for id in [
        FOLDERID_Windows,
        FOLDERID_ProgramFiles,
        FOLDERID_ProgramFilesX86,
        FOLDERID_ProgramData,
    ] {
        let mut value = std::ptr::null_mut();
        let result = unsafe { SHGetKnownFolderPath(&id, 0x4000, std::ptr::null_mut(), &mut value) };
        if result < 0 || value.is_null() {
            if !value.is_null() {
                unsafe { CoTaskMemFree(value.cast()) };
            }
            return Err(io::Error::other("System folder locations are unavailable."));
        }
        let mut length = 0;
        while length < 32768 && unsafe { *value.add(length) } != 0 {
            length += 1;
        }
        let path = (length < 32768).then(|| {
            PathBuf::from(std::ffi::OsString::from_wide(unsafe {
                std::slice::from_raw_parts(value, length)
            }))
        });
        unsafe { CoTaskMemFree(value.cast()) };
        roots.push(path.ok_or_else(budget)?);
    }
    Ok(roots)
}

#[cfg(target_os = "linux")]
fn system_roots() -> io::Result<Vec<PathBuf>> {
    Ok([
        "/bin",
        "/sbin",
        "/boot",
        "/dev",
        "/etc",
        "/lib",
        "/lib64",
        "/proc",
        "/sys",
        "/usr",
        "/run/systemd",
        "/run/udev",
        "/var/cache",
        "/var/db",
        "/var/lib",
        "/var/lock",
        "/var/log",
        "/var/run",
        "/var/spool",
    ]
    .map(PathBuf::from)
    .into())
}
#[cfg(target_os = "macos")]
fn system_roots() -> io::Result<Vec<PathBuf>> {
    Ok([
        "/System",
        "/Library",
        "/bin",
        "/sbin",
        "/usr",
        "/etc",
        "/private/etc",
        "/var/db",
        "/var/log",
        "/var/run",
        "/var/cache",
        "/private/var/db",
        "/private/var/log",
        "/private/var/run",
        "/private/var/cache",
    ]
    .map(PathBuf::from)
    .into())
}
#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn system_roots() -> io::Result<Vec<PathBuf>> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "System folder policy is unavailable on this platform.",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_root_entry_does_not_exclude_scope_enumeration() {
        let temporary = tempfile::Builder::new()
            .prefix("prism-policy-volume-")
            .tempdir()
            .unwrap();
        let volume = temporary.path().ancestors().last().unwrap();
        assert!(volume.file_name().is_none());
        let policy = Categories {
            hidden: false,
            system: false,
            ..Default::default()
        };
        let mut reader = Reader::new(policy, volume);
        // Directory roots carry implementation flags on real NTFS volumes.
        // A filter_entry(root) check must not prune the entire selected drive.
        assert_eq!(reader.check(volume, true).unwrap(), Decision::Include);
        #[cfg(windows)]
        assert_eq!(
            reader
                .check_windows_snapshot(volume, true, Some(0x16))
                .unwrap(),
            Decision::Include
        );
    }

    #[cfg(windows)]
    #[test]
    fn snapshot_attributes_avoid_leaf_stats_but_unknown_attributes_do_not() {
        let root = tempfile::Builder::new()
            .prefix("prism-policy-snapshot-")
            .tempdir()
            .unwrap();
        let missing = root.path().join("missing.docx");
        let policy = Categories {
            hidden: false,
            system: false,
            archives: false,
            ..Default::default()
        };
        let mut reader = Reader::new(policy, root.path());
        assert_eq!(
            reader
                .check_windows_snapshot(&missing, false, Some(0))
                .unwrap(),
            Decision::Include
        );
        assert_eq!(
            reader
                .check_windows_snapshot(&missing, false, Some(0x20 | 0x800 | 0x80000000))
                .unwrap(),
            Decision::Include
        );
        assert_eq!(
            reader
                .check_windows_snapshot(&missing, false, None)
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
        assert_eq!(
            reader.check(&missing, false).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        assert_eq!(
            reader
                .check_windows_snapshot(&missing, false, Some(2))
                .unwrap(),
            Decision::Exclude(Category::Hidden)
        );
        assert_eq!(
            reader
                .check_windows_snapshot(&missing, false, Some(4))
                .unwrap(),
            Decision::Exclude(Category::System)
        );
        assert_eq!(
            reader
                .check_windows_snapshot(&root.path().join(".dot.txt"), false, Some(0))
                .unwrap(),
            Decision::Exclude(Category::Hidden)
        );
        assert_eq!(
            reader
                .check_windows_snapshot(&missing.with_extension("zip"), false, Some(0))
                .unwrap(),
            Decision::Exclude(Category::Archives)
        );
        assert!(!missing.exists());
    }

    #[cfg(windows)]
    #[test]
    fn snapshot_attributes_do_not_bypass_scope_links_or_repository_rules() {
        let root = tempfile::Builder::new()
            .prefix("prism-policy-snapshot-")
            .tempdir()
            .unwrap();
        fs::create_dir(root.path().join(".git")).unwrap();
        fs::write(root.path().join(".gitignore"), "ignored.txt\n").unwrap();
        let policy = Categories {
            hidden: false,
            repository_ignored: false,
            ..Default::default()
        };
        let mut reader = Reader::new(policy, root.path());
        assert_eq!(
            reader
                .check_windows_snapshot(&root.path().join("ignored.txt"), false, Some(0))
                .unwrap(),
            Decision::Exclude(Category::RepositoryIgnored)
        );
        assert!(reader
            .check_windows_snapshot(&root.path().join("ordinary.txt"), false, Some(0x400))
            .is_err());
        assert!(reader
            .check_windows_snapshot(&root.path().join("../escape.txt"), false, Some(0))
            .is_err());
        assert!(reader
            .check_windows_snapshot(
                &root.path().with_file_name("outside-snapshot.txt"),
                false,
                Some(0)
            )
            .is_err());
        fs::create_dir(root.path().join(".private")).unwrap();
        assert_eq!(
            reader
                .check_windows_snapshot(&root.path().join(".private/leaf.txt"), false, Some(0))
                .unwrap(),
            Decision::Exclude(Category::Hidden)
        );
        let target = root.path().join("actual");
        let link = root.path().join("linked");
        fs::create_dir(&target).unwrap();
        crate::path_identity::junction(&link, &target);
        assert!(reader
            .check_windows_snapshot(&link.join("leaf.txt"), false, Some(0))
            .is_err());
        fs::remove_dir(&link).unwrap();
        assert!(target.is_dir());
    }

    #[test]
    fn directory_and_missing_rule_cache_eviction_preserves_exclusions() {
        let root = tempfile::Builder::new()
            .prefix("prism-policy-cache-")
            .tempdir()
            .unwrap();
        fs::create_dir(root.path().join(".git")).unwrap();
        fs::write(root.path().join(".gitignore"), "/denied/\n").unwrap();
        for name in [".private", "denied"] {
            fs::create_dir(root.path().join(name)).unwrap();
            fs::write(root.path().join(name).join("leaf.txt"), "not read").unwrap();
        }
        let mut reader = Reader::new(
            Categories {
                hidden: false,
                repository_ignored: false,
                ..Default::default()
            },
            root.path(),
        );
        for pass in 0..2 {
            assert_eq!(
                reader
                    .check(&root.path().join(".private/leaf.txt"), false)
                    .unwrap(),
                Decision::Exclude(Category::Hidden)
            );
            assert_eq!(
                reader
                    .check(&root.path().join("denied/leaf.txt"), false)
                    .unwrap(),
                Decision::Exclude(Category::RepositoryIgnored)
            );
            if pass == 0 {
                for i in 0..MAX_DIRECTORIES + 2 {
                    let directory = root.path().join(format!("ordinary-{i}"));
                    fs::create_dir(&directory).unwrap();
                    let path = directory.join("leaf.txt");
                    fs::write(&path, "not read").unwrap();
                    assert_eq!(reader.check(&path, false).unwrap(), Decision::Include);
                    assert!(reader.directories.len() <= MAX_DIRECTORIES);
                    assert!(reader.directory_key_bytes <= MAX_DIRECTORY_KEY_BYTES);
                }
            }
        }
    }

    #[test]
    fn unrestricted_and_name_only_policies_do_not_read_unavailable_files() {
        let root = tempfile::Builder::new()
            .prefix("prism-policy-")
            .tempdir()
            .unwrap();
        let missing = root.path().join("missing").join("code.RS");
        assert_eq!(
            Reader::new(Categories::default(), root.path())
                .check(&missing, false)
                .unwrap(),
            Decision::Include
        );
        let policy = Categories {
            source_code: false,
            ..Categories::default()
        };
        assert_eq!(
            Reader::new(policy, root.path())
                .check(&missing, false)
                .unwrap(),
            Decision::Exclude(Category::SourceCode)
        );
        assert_eq!(
            Reader::new(policy, root.path())
                .check(&missing.with_extension("txt"), false)
                .unwrap(),
            Decision::Include
        );
        let policy = Categories {
            hidden: false,
            ..Categories::default()
        };
        assert!(Reader::new(policy, root.path())
            .check(&missing, false)
            .is_err());
    }

    #[test]
    fn dot_ancestors_are_hidden_and_a_fresh_reader_observes_rule_changes() {
        let root = tempfile::Builder::new()
            .prefix("prism-policy-")
            .tempdir()
            .unwrap();
        let hidden = root.path().join(".private");
        fs::create_dir(&hidden).unwrap();
        let source = hidden.join("visible.txt");
        fs::write(&source, "never read as policy evidence").unwrap();
        let policy = Categories {
            hidden: false,
            ..Categories::default()
        };
        assert_eq!(
            Reader::new(policy, root.path())
                .check(&source, false)
                .unwrap(),
            Decision::Exclude(Category::Hidden)
        );
        let visible = root.path().join("public");
        fs::rename(&hidden, &visible).unwrap();
        assert_eq!(
            Reader::new(policy, root.path())
                .check(&visible.join("visible.txt"), false)
                .unwrap(),
            Decision::Include
        );
        assert!(Reader::new(policy, root.path())
            .check(&root.path().join("public/../public/visible.txt"), false)
            .is_err());
    }

    #[cfg(windows)]
    #[test]
    fn actual_windows_hidden_and_system_ancestors_do_not_hide_an_entire_volume() {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{GetFileAttributesW, SetFileAttributesW};
        let root = tempfile::Builder::new()
            .prefix("prism-policy-")
            .tempdir()
            .unwrap();
        let folder = root.path().join("ordinary");
        fs::create_dir(&folder).unwrap();
        let source = folder.join("evidence.txt");
        fs::write(&source, "body not inspected").unwrap();
        let wide: Vec<_> = folder.as_os_str().encode_wide().chain(Some(0)).collect();
        let original = unsafe { GetFileAttributesW(wide.as_ptr()) };
        assert_ne!(original, u32::MAX);
        assert_ne!(
            unsafe { SetFileAttributesW(wide.as_ptr(), original | 6) },
            0
        );
        for (policy, expected) in [
            (
                Categories {
                    hidden: false,
                    ..Categories::default()
                },
                Category::Hidden,
            ),
            (
                Categories {
                    system: false,
                    ..Categories::default()
                },
                Category::System,
            ),
        ] {
            assert_eq!(
                Reader::new(policy, root.path())
                    .check(&source, false)
                    .unwrap(),
                Decision::Exclude(expected)
            );
        }
        assert_ne!(unsafe { SetFileAttributesW(wide.as_ptr(), original) }, 0);
        let policy = Categories {
            hidden: false,
            system: false,
            ..Categories::default()
        };
        assert_eq!(
            Reader::new(policy, root.path())
                .check(&source, false)
                .unwrap(),
            Decision::Include
        );
    }

    #[test]
    fn system_roots_use_component_boundaries() {
        let root = PathBuf::from(if cfg!(windows) { r"C:\System" } else { "/usr" });
        assert!(within(&root.join("lib"), &root));
        assert!(!within(
            &PathBuf::from(format!("{}-user/file", root.display())),
            &root
        ));
        #[cfg(windows)]
        assert!(within(Path::new("c:/sYsTeM/sub/file"), &root));
    }

    #[cfg(unix)]
    #[test]
    fn known_system_locations_keep_user_homes_portals_and_temporary_documents_distinct() {
        let roots = system_roots().unwrap();
        for path in [
            "/var/home/person/document.txt",
            "/var/mnt/drive/project.txt",
            "/run/user/1000/doc/portal.txt",
            "/private/var/folders/person/T/draft.txt",
        ] {
            assert!(
                !roots.iter().any(|root| within(Path::new(path), root)),
                "{path}"
            );
        }
        for path in ["/usr/lib/runtime", "/var/log/system.log"] {
            assert!(
                roots.iter().any(|root| within(Path::new(path), root)),
                "{path}"
            );
        }
    }
}

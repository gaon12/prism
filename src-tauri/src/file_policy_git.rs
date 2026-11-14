//! Hierarchical repository-local rules, supplied to the upstream matcher from
//! bounded, no-follow reads. No Git execution or global configuration lookup.
use super::{budget, plain_metadata, unsafe_path, within};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use std::{
    collections::{HashSet, VecDeque},
    fs::{self, File, OpenOptions},
    io::{self, Read},
    path::{Path, PathBuf},
    sync::Arc,
};

const MAX_RULE_BYTES: u64 = 64 * 1024;
const MAX_RULE_LINES: usize = 1024;
const MAX_ACTIVE_FILES: usize = 16;
const MAX_ACTIVE_BYTES: usize = 256 * 1024;
const MAX_CACHED_FILES: usize = 16;

#[derive(Clone, Copy)]
pub(super) enum Marker {
    Directory,
    File,
}

pub(super) fn marker(directory: &Path) -> io::Result<Option<Marker>> {
    match plain_metadata(&directory.join(".git")) {
        Ok(metadata) if metadata.is_dir() => Ok(Some(Marker::Directory)),
        Ok(metadata) if metadata.is_file() => Ok(Some(Marker::File)),
        Ok(_) => Err(unsafe_path()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

pub(super) fn repository_start(root: &Path) -> io::Result<PathBuf> {
    for (depth, path) in root.ancestors().enumerate() {
        if depth > super::MAX_DEPTH {
            return Err(budget());
        }
        if marker(path)?.is_some() {
            return Ok(path.to_owned());
        }
    }
    Ok(root.to_owned())
}

struct Compiled {
    root: PathBuf,
    matcher: Gitignore,
    bytes: usize,
}

#[derive(Default)]
pub(super) struct Active {
    repository: bool,
    rules: Vec<Arc<Compiled>>,
    bytes: usize,
}

impl Active {
    pub(super) fn in_repository(&self) -> bool {
        self.repository
    }

    pub(super) fn ignored(&self, path: &Path, is_dir: bool) -> bool {
        for rule in self.rules.iter().rev() {
            if !within(path, &rule.root) {
                continue;
            }
            // Keep pattern matching case-sensitive while handling Windows
            // root spelling through the app's existing comparison convention.
            // The matcher is rooted at '.', so no absolute prefix is stripped.
            let root = crate::path_identity::normal(&rule.root);
            let path = crate::path_identity::normal(path);
            let relative: PathBuf = path.components().skip(root.components().count()).collect();
            let matched = rule.matcher.matched(&relative, is_dir);
            if !matched.is_none() {
                return matched.is_ignore();
            }
        }
        false
    }

    fn push(&mut self, rule: Option<Arc<Compiled>>) -> io::Result<()> {
        if let Some(rule) = rule {
            if !rule.matcher.is_empty() {
                self.bytes = self.bytes.saturating_add(rule.bytes);
                if self.rules.len() >= MAX_ACTIVE_FILES || self.bytes > MAX_ACTIVE_BYTES {
                    return Err(budget());
                }
                self.rules.push(rule);
            }
        }
        Ok(())
    }
}

#[derive(Default)]
pub(super) struct Rules {
    cache: VecDeque<(PathBuf, PathBuf, Option<Arc<Compiled>>)>,
    missing: HashSet<PathBuf>,
    missing_key_bytes: usize,
}

impl Rules {
    pub(super) fn repository(&mut self, root: &Path, marker: Marker) -> io::Result<Active> {
        let entry = root.join(".git");
        let git_dir = match marker {
            Marker::Directory => entry,
            Marker::File => {
                let data = read_small(&entry, 4096)?.ok_or_else(unsafe_path)?;
                let target = data
                    .trim()
                    .strip_prefix("gitdir: ")
                    .ok_or_else(unsafe_path)?;
                resolve_directory(root, target)?
            }
        };
        let common = match read_small(&git_dir.join("commondir"), 4096)? {
            Some(data) => resolve_directory(&git_dir, data.trim())?,
            None => git_dir,
        };
        let mut active = Active {
            repository: true,
            ..Active::default()
        };
        active.push(self.load(root, &common.join("info/exclude"))?)?;
        Ok(active)
    }

    pub(super) fn add_directory(
        &mut self,
        active: &mut Active,
        directory: &Path,
    ) -> io::Result<()> {
        active.push(self.load(directory, &directory.join(".gitignore"))?)
    }

    fn load(&mut self, root: &Path, file: &Path) -> io::Result<Option<Arc<Compiled>>> {
        if self.missing.contains(file) {
            return Ok(None);
        }
        if let Some(index) = self
            .cache
            .iter()
            .position(|(base, path, _)| base == root && path == file)
        {
            let entry = self.cache.remove(index).unwrap();
            let result = entry.2.clone();
            self.cache.push_back(entry);
            return Ok(result);
        }
        let compiled = match read_small(file, MAX_RULE_BYTES)? {
            Some(data) => {
                let mut builder = GitignoreBuilder::new(".");
                for (index, line) in data.trim_start_matches('\u{feff}').lines().enumerate() {
                    if index >= MAX_RULE_LINES {
                        return Err(budget());
                    }
                    builder.add_line(Some(file.to_owned()), line).map_err(|_| {
                        io::Error::new(
                            io::ErrorKind::InvalidData,
                            "Repository ignore syntax is invalid.",
                        )
                    })?;
                }
                Some(Arc::new(Compiled {
                    root: root.to_owned(),
                    bytes: data.len(),
                    matcher: builder.build().map_err(|_| budget())?,
                }))
            }
            None => {
                let bytes = file.as_os_str().len();
                if self.missing.len() >= super::MAX_DIRECTORIES
                    || self.missing_key_bytes.saturating_add(bytes) > super::MAX_DIRECTORY_KEY_BYTES
                {
                    self.missing.clear();
                    self.missing_key_bytes = 0;
                }
                self.missing.insert(file.to_owned());
                self.missing_key_bytes += bytes;
                return Ok(None);
            }
        };
        if self.cache.len() >= MAX_CACHED_FILES {
            self.cache.pop_front();
        }
        self.cache
            .push_back((root.to_owned(), file.to_owned(), compiled.clone()));
        Ok(compiled)
    }
}

fn resolve_directory(base: &Path, value: &str) -> io::Result<PathBuf> {
    if value.is_empty() || value.contains(['\0', '\r', '\n']) {
        return Err(unsafe_path());
    }
    let path = crate::path_identity::canonical(&base.join(value))?;
    if !plain_metadata(&path)?.is_dir() {
        return Err(unsafe_path());
    }
    Ok(path)
}

fn read_small(path: &Path, limit: u64) -> io::Result<Option<String>> {
    let before = match plain_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if !before.is_file() || before.len() > limit {
        return Err(budget());
    }
    crate::path_identity::no_links(path)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x0020_0000).share_mode(1);
    }
    let mut file = options.open(path)?;
    let opened = file.metadata()?;
    if !opened.is_file() || !same_entry(&before, &opened) || opened.len() > limit {
        return Err(unsafe_path());
    }
    single_link(&file, &opened)?;
    let mut data = Vec::new();
    file.by_ref().take(limit + 1).read_to_end(&mut data)?;
    let after = file.metadata()?;
    let current = plain_metadata(path)?;
    if data.len() as u64 > limit
        || !same_entry(&opened, &after)
        || !same_entry(&after, &current)
        || opened.len() != after.len()
        || opened.modified()? != after.modified()?
    {
        return Err(budget());
    }
    crate::path_identity::no_links(path)?;
    String::from_utf8(data).map(Some).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "Repository ignore rules are not UTF-8.",
        )
    })
}

fn same_entry(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if left.dev() != right.dev()
            || left.ino() != right.ino()
            || left.ctime() != right.ctime()
            || left.ctime_nsec() != right.ctime_nsec()
        {
            return false;
        }
    }
    left.is_file() == right.is_file()
        && left.len() == right.len()
        && left.modified().ok() == right.modified().ok()
}

fn single_link(file: &File, metadata: &fs::Metadata) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let _ = file;
        if metadata.nlink() != 1 {
            return Err(unsafe_path());
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::{fs::MetadataExt, io::AsRawHandle};
        use windows_sys::Win32::Storage::FileSystem::{
            GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
        };
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if info.nNumberOfLinks != 1 || metadata.file_attributes() & 0x400 != 0 {
            return Err(unsafe_path());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        file_policy::{Categories, Category, Decision},
        file_policy_fs::Reader,
    };

    fn policy() -> Categories {
        Categories {
            repository_ignored: false,
            ..Categories::default()
        }
    }
    fn fixture() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join(".git")).unwrap();
        root
    }
    fn excluded(reader: &mut Reader, root: &Path, relative: &str, is_dir: bool) -> bool {
        let path = root.join(relative);
        if is_dir {
            fs::create_dir_all(&path).unwrap();
        } else {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, [0xff, 0, 1]).unwrap();
        }
        reader.check(&path, is_dir).unwrap() == Decision::Exclude(Category::RepositoryIgnored)
    }

    #[test]
    fn hierarchy_parent_pruning_nested_repositories_and_literal_patterns() {
        let root = fixture();
        fs::write(root.path().join(".gitignore"), "\u{feff}*.log\r\n!keep.log\r\n/root-only.txt\r\nblocked/\r\n!blocked/reopen.txt\r\nbuild/**\r\n!build/keep/\r\n!build/keep/**\r\n\\#literal\r\n\\!literal\r\nspace\\ \r\n").unwrap();
        fs::create_dir_all(root.path().join("sub")).unwrap();
        fs::write(root.path().join("sub/.gitignore"), "!local.log\n").unwrap();
        fs::create_dir_all(root.path().join("blocked/.git")).unwrap();
        fs::write(root.path().join("blocked/.gitignore"), "!reopen.txt\n").unwrap();
        let mut reader = Reader::new(policy(), root.path());
        for (path, expected) in [
            ("a.log", true),
            ("keep.log", false),
            ("sub/a.log", true),
            ("sub/local.log", false),
            ("root-only.txt", true),
            ("sub/root-only.txt", false),
            ("blocked/reopen.txt", true),
            ("build/a.txt", true),
            ("build/keep/a.txt", false),
            ("#literal", true),
            ("!literal", true),
        ] {
            assert_eq!(
                excluded(&mut reader, root.path(), path, false),
                expected,
                "{path}"
            );
        }
        assert!(excluded(&mut reader, root.path(), "build/drop", true));
    }

    #[test]
    fn info_exclude_is_lower_priority_and_worktrees_resolve_common_metadata() {
        let root = fixture();
        fs::create_dir(root.path().join(".git/info")).unwrap();
        fs::write(root.path().join(".git/info/exclude"), "*.cache\n").unwrap();
        fs::write(root.path().join(".gitignore"), "!keep.cache\n").unwrap();
        let mut reader = Reader::new(policy(), root.path());
        assert!(excluded(&mut reader, root.path(), "drop.cache", false));
        assert!(!excluded(&mut reader, root.path(), "keep.cache", false));
        let tree = root.path().join("tree");
        fs::create_dir(&tree).unwrap();
        fs::create_dir(root.path().join(".git/worktree")).unwrap();
        fs::write(root.path().join(".git/worktree/commondir"), "..\n").unwrap();
        fs::write(tree.join(".git"), "gitdir: ../.git/worktree\n").unwrap();
        fs::write(tree.join(".gitignore"), "!keep.cache\n").unwrap();
        let mut reader = Reader::new(policy(), &tree);
        assert!(excluded(&mut reader, &tree, "drop.cache", false));
        assert!(!excluded(&mut reader, &tree, "keep.cache", false));
    }

    #[test]
    fn a_new_reader_reloads_rules_and_linked_or_oversized_rules_are_errors() {
        let root = fixture();
        let rules = root.path().join(".gitignore");
        fs::write(&rules, "a.txt\n").unwrap();
        let mut reader = Reader::new(policy(), root.path());
        assert!(excluded(&mut reader, root.path(), "a.txt", false));
        fs::write(&rules, "b.txt\n").unwrap();
        assert!(excluded(&mut reader, root.path(), "a.txt", false));
        assert!(!excluded(
            &mut Reader::new(policy(), root.path()),
            root.path(),
            "a.txt",
            false
        ));
        let peer = root.path().join("peer.txt");
        fs::hard_link(&rules, &peer).unwrap();
        assert!(Reader::new(policy(), root.path())
            .check(&peer, false)
            .is_err());
        fs::remove_file(&peer).unwrap();
        fs::write(&rules, vec![b'a'; MAX_RULE_BYTES as usize + 1]).unwrap();
        assert!(Reader::new(policy(), root.path())
            .check(&root.path().join("a.txt"), false)
            .is_err());
    }

    #[cfg(unix)]
    #[test]
    fn linked_rules_and_linked_ancestor_directories_are_rejected() {
        use std::os::unix::fs::symlink;
        let root = fixture();
        let other = root.path().join("other");
        fs::create_dir(&other).unwrap();
        let rule = other.join("rule");
        fs::write(&rule, "*.txt\n").unwrap();
        symlink(&rule, root.path().join(".gitignore")).unwrap();
        assert!(Reader::new(policy(), root.path())
            .check(&root.path().join("x.txt"), false)
            .is_err());
        fs::remove_file(root.path().join(".gitignore")).unwrap();
        symlink(&other, root.path().join("linked")).unwrap();
        assert!(Reader::new(policy(), root.path())
            .check(&root.path().join("linked/x.txt"), false)
            .is_err());
    }

    #[cfg(windows)]
    #[test]
    fn junction_rule_parents_are_rejected_without_reading_the_target_rules() {
        let root = fixture();
        let other = root.path().join(".git").join("other");
        fs::create_dir(&other).unwrap();
        fs::write(other.join("exclude"), "*.txt\n").unwrap();
        crate::path_identity::junction(&root.path().join(".git").join("info"), &other);
        assert!(Reader::new(policy(), root.path())
            .check(&root.path().join("x.txt"), false)
            .is_err());
    }

    #[test]
    fn malformed_rules_are_unavailable_only_when_the_policy_requires_them() {
        let root = fixture();
        fs::write(root.path().join(".gitignore"), [0xff, 0xfe]).unwrap();
        let source = root.path().join("source.txt");
        fs::write(&source, "candidate contents must not be inspected").unwrap();
        assert!(Reader::new(policy(), root.path())
            .check(&source, false)
            .is_err());
        assert_eq!(
            Reader::new(Categories::default(), root.path())
                .check(&source, false)
                .unwrap(),
            Decision::Include
        );
    }

    #[test]
    #[ignore = "Opt-in comparison with an installed Git in an isolated owned repository"]
    fn matches_git_check_ignore_no_index_for_repository_local_patterns() {
        use std::{
            process::{Command, Stdio},
            time::{Duration, Instant},
        };
        fn git(root: &Path, args: &[&str]) -> i32 {
            let mut command = Command::new("git");
            command
                .current_dir(root)
                .arg("--git-dir")
                .arg(root.join(".git"))
                .arg("--work-tree")
                .arg(root)
                .args(["-c", "core.ignorecase=false"])
                .args(args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            for (key, _) in std::env::vars_os() {
                if key.to_string_lossy().starts_with("GIT_") {
                    command.env_remove(key);
                }
            }
            command
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", root.join("isolated.config"))
                .env("GIT_TEMPLATE_DIR", root.join("empty-template"))
                .env("GIT_TERMINAL_PROMPT", "0");
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                command.creation_flags(0x0800_0000);
            }
            let mut child = command.spawn().expect("The opt-in oracle requires Git");
            let start = Instant::now();
            loop {
                if let Some(status) = child.try_wait().unwrap() {
                    return status.code().unwrap();
                }
                if start.elapsed() > Duration::from_secs(5) {
                    child.kill().unwrap();
                    child.wait().unwrap();
                    panic!("Owned Git oracle exceeded its deadline");
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        let root = fixture();
        fs::write(root.path().join("isolated.config"), "").unwrap();
        fs::create_dir(root.path().join("empty-template")).unwrap();
        assert_eq!(git(root.path(), &["init", "--quiet"]), 0);
        fs::create_dir_all(root.path().join(".git/info")).unwrap();
        fs::write(root.path().join(".git/info/exclude"), "*.cache\n").unwrap();
        fs::write(root.path().join(".gitignore"), "*.log\n!keep.log\n/root-only.txt\nblocked/\n!blocked/reopen.txt\nbuild/**\n!build/keep/\n!build/keep/**\n\\#literal\n\\!literal\na/**/z.txt\n!keep.cache\n").unwrap();
        fs::create_dir(root.path().join("sub")).unwrap();
        fs::write(root.path().join("sub/.gitignore"), "!local.log\n").unwrap();
        let mut reader = Reader::new(policy(), root.path());
        for path in [
            "a.log",
            "keep.log",
            "sub/a.log",
            "sub/local.log",
            "root-only.txt",
            "sub/root-only.txt",
            "blocked/reopen.txt",
            "build/a.txt",
            "build/keep/a.txt",
            "#literal",
            "!literal",
            "a/z.txt",
            "a/b/z.txt",
            "a/b/y.txt",
            "drop.cache",
            "keep.cache",
        ] {
            let actual = excluded(&mut reader, root.path(), path, false);
            let status = git(
                root.path(),
                &["check-ignore", "--no-index", "--quiet", "--", path],
            );
            assert!(status == 0 || status == 1);
            assert_eq!(actual, status == 0, "{path}");
        }
    }
}

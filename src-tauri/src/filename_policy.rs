//! Per-pass filename inclusion facts. Scope authorization stays with FileIndex.
//! Keep one root reader at a time so many configured scopes cannot multiply
//! its bounded caches. Default inclusion allocates/reads no per-path facts.
use super::{path_key, under, Root};
use crate::file_policy::{Categories, Decision};
use crate::file_policy_fs::Reader;
use std::{io, path::Path};

/// Repository-local rule changes affect the folder, not just the rule entry.
/// The caller clamps this target to active filename roots before enumeration.
pub(super) fn watch_root(path: &Path, categories: Categories) -> Option<std::path::PathBuf> {
    if categories.repository_ignored {
        return None;
    }
    let named = |path: &Path, expected: &str| {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                #[cfg(windows)]
                {
                    name.eq_ignore_ascii_case(expected)
                }
                #[cfg(not(windows))]
                {
                    name == expected
                }
            })
    };
    if named(path, ".gitignore") || named(path, ".git") {
        return path.parent().map(Path::to_owned);
    }
    let parent = path.parent()?;
    if named(path, "commondir") && named(parent, ".git") {
        return parent.parent().map(Path::to_owned);
    }
    if named(path, "exclude") && named(parent, "info") {
        let git = parent.parent()?;
        if named(git, ".git") {
            return git.parent().map(Path::to_owned);
        }
    }
    None
}

pub(super) struct Pass<'a> {
    categories: Categories,
    roots: &'a [Root],
    reader: Option<(usize, Reader)>,
}
impl<'a> Pass<'a> {
    pub(super) fn new(categories: Categories, roots: &'a [Root]) -> Self {
        Self {
            categories,
            roots,
            reader: None,
        }
    }
    fn reader(&mut self, path: &Path) -> io::Result<&mut Reader> {
        let key = path_key(path);
        let (index, root) = self
            .roots
            .iter()
            .enumerate()
            .filter(|(_, root)| under(&key, &root.key))
            .max_by_key(|(_, root)| root.key.len())
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::PermissionDenied, "No filename policy root")
            })?;
        if self
            .reader
            .as_ref()
            .is_none_or(|(previous, _)| *previous != index)
        {
            self.reader = Some((index, Reader::new(self.categories, &root.path)));
        }
        Ok(&mut self.reader.as_mut().expect("selected reader").1)
    }
    pub(super) fn check(&mut self, path: &Path, is_dir: bool) -> io::Result<bool> {
        if self.categories.unrestricted() {
            return Ok(true);
        }
        self.reader(path)?
            .check(path, is_dir)
            .map(|value| value == Decision::Include)
    }
    pub(super) fn snapshot(
        &mut self,
        path: &Path,
        is_dir: bool,
        attributes: Option<u32>,
    ) -> io::Result<bool> {
        if self.categories.unrestricted() {
            return Ok(true);
        }
        #[cfg(windows)]
        {
            self.reader(path)?
                .check_windows_snapshot(path, is_dir, attributes)
                .map(|value| value == Decision::Include)
        }
        #[cfg(not(windows))]
        {
            let _ = attributes;
            self.check(path, is_dir)
        }
    }
}

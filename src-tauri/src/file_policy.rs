//! Shared Q18 inclusion rules. These narrow an authorized scope; they never
//! grant permission to list a path, read content or send it to a provider.
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Categories {
    pub hidden: bool,
    pub system: bool,
    pub source_code: bool,
    pub repository_ignored: bool,
    pub archives: bool,
}

impl Default for Categories {
    fn default() -> Self {
        Self {
            hidden: true,
            system: true,
            source_code: true,
            repository_ignored: true,
            archives: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Preferences {
    pub filename: Categories,
    pub content: Categories,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Hidden,
    System,
    SourceCode,
    RepositoryIgnored,
    Archives,
}

/// Facts about the complete path, not just the last directory entry. Hidden
/// and system facts must include relevant ancestors. RepositoryIgnored must
/// reflect hierarchical repository rules, not a fixed directory-name list.
#[derive(Clone, Copy, Debug, Default)]
pub struct Facts {
    pub hidden: Option<bool>,
    pub system: Option<bool>,
    pub source_code: Option<bool>,
    pub repository_ignored: Option<bool>,
    pub archives: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Include,
    Exclude(Category),
    /// Resolve this fact before admission. Missing facts never mean false.
    NeedsEvidence(Category),
}

impl Categories {
    pub fn unrestricted(self) -> bool {
        self == Self::default()
    }

    pub fn needs_path_attributes(self) -> bool {
        !self.hidden || !self.system
    }

    pub fn decide(self, facts: &Facts) -> Decision {
        let mut missing = None;
        for (category, included, fact) in [
            (Category::Hidden, self.hidden, facts.hidden),
            (Category::System, self.system, facts.system),
            (Category::SourceCode, self.source_code, facts.source_code),
            (
                Category::RepositoryIgnored,
                self.repository_ignored,
                facts.repository_ignored,
            ),
            (Category::Archives, self.archives, facts.archives),
        ] {
            if included {
                continue;
            }
            match fact {
                Some(true) => return Decision::Exclude(category),
                Some(false) => {}
                None => {
                    missing.get_or_insert(category);
                }
            }
        }
        missing.map_or(Decision::Include, Decision::NeedsEvidence)
    }
}

impl Facts {
    /// Only filename-derived facts. No filesystem queries, ignore-file reads,
    /// archive inspection or inferred hidden/system flags occur here.
    pub fn from_name(path: &Path, is_dir: bool) -> Self {
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("");
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("");
        let source_code = !is_dir
            && (SOURCE_EXTENSIONS
                .iter()
                .any(|value| extension.eq_ignore_ascii_case(value))
                || SOURCE_NAMES
                    .iter()
                    .any(|value| name.eq_ignore_ascii_case(value)));
        let archives = !is_dir
            && ARCHIVE_EXTENSIONS
                .iter()
                .any(|value| extension.eq_ignore_ascii_case(value));
        Self {
            source_code: Some(source_code),
            archives: Some(archives),
            ..Self::default()
        }
    }
}

// Language sources and build scripts. General text/data/documents (.md, .txt,
// .json, .xml, .docx, .hwpx, etc.) are not classified as source by their format.
const SOURCE_EXTENSIONS: &[&str] = &[
    "rs", "c", "h", "cc", "hh", "cpp", "hpp", "cxx", "hxx", "m", "mm", "cs", "fs", "fsx", "vb",
    "go", "java", "kt", "kts", "scala", "swift", "py", "pyw", "pyi", "rb", "php", "pl", "pm", "r",
    "lua", "jl", "dart", "js", "jsx", "mjs", "cjs", "ts", "tsx", "mts", "cts", "vue", "svelte",
    "html", "htm", "css", "scss", "sass", "less", "sql", "sh", "bash", "zsh", "fish", "ps1",
    "psm1", "psd1", "bat", "cmd", "ex", "exs", "erl", "hrl", "clj", "cljs", "cljc", "edn", "hs",
    "lhs", "ml", "mli", "elm", "zig", "nim", "nims", "v", "sv", "vhd", "vhdl", "asm", "s", "cmake",
    "gradle",
];
const SOURCE_NAMES: &[&str] = &[
    "Makefile",
    "GNUmakefile",
    "CMakeLists.txt",
    "Dockerfile",
    "Containerfile",
    "Justfile",
    "meson.build",
    "meson_options.txt",
];
// Archive files, not every ZIP-based document container. This never enables
// archive-content extraction or changes collection/reference permissions.
const ARCHIVE_EXTENSIONS: &[&str] = &[
    "zip", "7z", "rar", "tar", "gz", "gzip", "tgz", "bz2", "bzip2", "tbz", "tbz2", "xz", "txz",
    "zst", "zstd", "tzst", "lz", "lzma", "lzo", "br", "cab", "arj", "lz4", "jar", "war", "ear",
    "whl", "gem", "cbz", "cbr", "cb7", "cbt",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_partial_and_round_trip_preferences_keep_independent_defaults() {
        for json in ["{}", r#"{"filename":{}}"#, r#"{"content":{}}"#] {
            assert_eq!(
                serde_json::from_str::<Preferences>(json).unwrap(),
                Preferences::default()
            );
        }
        let preferences: Preferences = serde_json::from_str(r#"{"filename":{"hidden":false,"archives":false},"content":{"sourceCode":false,"repositoryIgnored":false}}"#).unwrap();
        assert!(!preferences.filename.hidden);
        assert!(!preferences.filename.archives);
        assert!(preferences.filename.source_code && preferences.filename.repository_ignored);
        assert!(preferences.content.hidden && preferences.content.archives);
        assert!(!preferences.content.source_code && !preferences.content.repository_ignored);
        assert_eq!(
            serde_json::from_str::<Preferences>(&serde_json::to_string(&preferences).unwrap())
                .unwrap(),
            preferences
        );
        assert!(serde_json::from_str::<Preferences>(r#"{"filename":{"hidden":"false"}}"#).is_err());
    }

    #[test]
    fn default_inclusion_needs_no_metadata_and_exclusion_requires_known_facts() {
        assert!(Categories::default().unrestricted());
        assert!(!Categories::default().needs_path_attributes());
        assert_eq!(
            Categories::default().decide(&Facts::default()),
            Decision::Include
        );
        for category in [
            Category::Hidden,
            Category::System,
            Category::SourceCode,
            Category::RepositoryIgnored,
            Category::Archives,
        ] {
            let mut policy = Categories::default();
            let mut facts = Facts::default();
            let (setting, fact) = match category {
                Category::Hidden => (&mut policy.hidden, &mut facts.hidden),
                Category::System => (&mut policy.system, &mut facts.system),
                Category::SourceCode => (&mut policy.source_code, &mut facts.source_code),
                Category::RepositoryIgnored => (
                    &mut policy.repository_ignored,
                    &mut facts.repository_ignored,
                ),
                Category::Archives => (&mut policy.archives, &mut facts.archives),
            };
            *setting = false;
            *fact = Some(false);
            assert!(!policy.unrestricted());
            assert_eq!(
                policy.decide(&Facts::default()),
                Decision::NeedsEvidence(category)
            );
            assert_eq!(policy.decide(&facts), Decision::Include);
            match category {
                Category::Hidden => facts.hidden = Some(true),
                Category::System => facts.system = Some(true),
                Category::SourceCode => facts.source_code = Some(true),
                Category::RepositoryIgnored => facts.repository_ignored = Some(true),
                Category::Archives => facts.archives = Some(true),
            }
            assert_eq!(policy.decide(&facts), Decision::Exclude(category));
        }
        let policy = Categories {
            hidden: false,
            archives: false,
            ..Categories::default()
        };
        assert!(policy.needs_path_attributes());
        assert_eq!(
            policy.decide(&Facts::from_name(Path::new("backup.tar.gz"), false)),
            Decision::Exclude(Category::Archives)
        );
    }

    #[test]
    fn name_rules_distinguish_code_archives_documents_and_directories() {
        for name in [
            "한국어.RS",
            "client.TSX",
            "script.ps1",
            "CMakeLists.txt",
            "Dockerfile",
        ] {
            assert_eq!(
                Facts::from_name(Path::new(name), false).source_code,
                Some(true)
            );
            assert_eq!(
                Facts::from_name(Path::new(name), true).source_code,
                Some(false)
            );
        }
        for name in [
            "report.md",
            "requirements.txt",
            "data.json",
            "source.rs.txt",
            "notes.hwpx",
        ] {
            assert_eq!(
                Facts::from_name(Path::new(name), false).source_code,
                Some(false)
            );
        }
        for name in ["backup.TAR.GZ", "comic.cbz", "package.whl", "data.7z"] {
            assert_eq!(
                Facts::from_name(Path::new(name), false).archives,
                Some(true)
            );
            assert_eq!(
                Facts::from_name(Path::new(name), true).archives,
                Some(false)
            );
        }
        for name in ["report.docx", "book.epub", "form.hwpx", "compressed.txt"] {
            let facts = Facts::from_name(Path::new(name), false);
            assert_eq!(facts.archives, Some(false));
            assert!(
                facts.hidden.is_none()
                    && facts.system.is_none()
                    && facts.repository_ignored.is_none()
            );
        }
    }
}

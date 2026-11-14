//! Connect conversation evidence to the current local content index. Stored chat
//! provenance never grants file access by itself.
use crate::{
    content_service::ContentService,
    content_store::ContentChunk,
    rag_service::{
        CheckedSource, DocumentRetrieval, RagError, RagErrorCode, RagPolicy, RagResult, RagSource,
        SourceRef, SourceResolver, SourceState,
    },
};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::Read,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

pub struct ContentSources(pub ContentService);
fn key(path: &Path) -> String {
    let value = path.to_string_lossy().replace('\\', "/");
    let value = if let Some(rest) = value.strip_prefix("//?/UNC/") {
        format!("//{rest}")
    } else {
        value.strip_prefix("//?/").unwrap_or(&value).to_owned()
    };
    #[cfg(windows)]
    let value = value.to_lowercase();
    value.trim_end_matches('/').to_owned()
}
fn within(path: &str, root: &str) -> bool {
    path == root
        || path
            .strip_prefix(root)
            .is_some_and(|tail| tail.starts_with('/'))
}
fn cancel_check(cancel: &AtomicBool) -> RagResult<()> {
    if cancel.load(Ordering::Acquire) {
        Err(RagError::new(RagErrorCode::Cancelled))
    } else {
        Ok(())
    }
}
fn no_links(path: &Path) -> bool {
    crate::path_identity::no_links(path).is_ok()
}

fn selected(policy: &RagPolicy, ids: &[String]) -> RagResult<Vec<(String, String)>> {
    if ids.iter().any(|id| {
        !policy
            .scopes
            .iter()
            .any(|scope| scope.id == *id && scope.content_enabled)
    }) {
        return Err(RagError::new(RagErrorCode::ScopeBlocked));
    }
    Ok(policy
        .scopes
        .iter()
        .filter(|scope| scope.content_enabled && (ids.is_empty() || ids.contains(&scope.id)))
        .filter_map(|scope| {
            let path = Path::new(&scope.path);
            if !no_links(path) {
                return None;
            }
            let canonical = fs::canonicalize(path).ok()?;
            canonical
                .is_dir()
                .then(|| (scope.id.clone(), key(&canonical)))
        })
        .collect())
}
fn scope_for(path: &Path, policy: &RagPolicy, selected: &[(String, String)]) -> Option<String> {
    if !no_links(path) {
        return None;
    }
    let canonical = crate::path_identity::canonical(path).ok()?;
    let path_key = key(&canonical);
    if !canonical.is_file() || !selected.iter().any(|(_, root)| within(&path_key, root)) {
        return None;
    }
    let mut scopes: Vec<_> = policy
        .scopes
        .iter()
        .filter(|scope| scope.content_enabled)
        .filter_map(|scope| {
            let root = Path::new(&scope.path);
            if !no_links(root) {
                return None;
            }
            let canonical = fs::canonicalize(root).ok()?;
            let root = key(&canonical);
            (canonical.is_dir() && within(&path_key, &root)).then_some((
                scope.id.clone(),
                root.len(),
                canonical,
            ))
        })
        .collect();
    scopes.sort_by_key(|scope| std::cmp::Reverse(scope.1));
    let first = scopes.first()?;
    if scopes.get(1).is_some_and(|other| other.1 == first.1) {
        return None;
    }
    if !policy.file_policy.unrestricted()
        && crate::file_policy_fs::Reader::new(policy.file_policy, &first.2)
            .check(&canonical, false)
            .ok()
            != Some(crate::file_policy::Decision::Include)
    {
        return None;
    }
    Some(first.0.clone())
}
// Hash the current authorized file once per preparation pass, even if size and
// modification time were preserved by the editor. No source text is logged.
fn current_sha(path: &Path, cancel: &AtomicBool) -> RagResult<Option<String>> {
    #[cfg(test)]
    SOURCE_HASH_REQUESTS.with(|calls| calls.set(calls.get() + 1));
    cancel_check(cancel)?;
    if !no_links(path) {
        return Ok(None);
    }
    let Ok(mut file) = File::open(path) else {
        return Ok(None);
    };
    let Ok(before) = file.metadata() else {
        return Ok(None);
    };
    if !before.is_file() {
        return Ok(None);
    }
    if before.len() > 64 * 1024 * 1024 {
        return Err(RagError::new(RagErrorCode::Limit));
    }
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut bytes = 0u64;
    loop {
        cancel_check(cancel)?;
        let Ok(count) = file.read(&mut buffer) else {
            return Ok(None);
        };
        if count == 0 {
            break;
        }
        bytes += count as u64;
        if bytes > 64 * 1024 * 1024 {
            return Err(RagError::new(RagErrorCode::Limit));
        }
        hash.update(&buffer[..count]);
    }
    let Ok(after) = file.metadata() else {
        return Ok(None);
    };
    if bytes != before.len()
        || after.len() != before.len()
        || after.modified().ok() != before.modified().ok()
        || !no_links(path)
    {
        return Ok(None);
    }
    Ok(Some(format!("{:x}", hash.finalize())))
}

#[cfg(test)]
thread_local! {
    // Thread-local so parallel fixtures cannot affect the read-gate assertion.
    static SOURCE_HASH_REQUESTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rag_service::{GenerationPreferences, RagScope};

    fn policy_fixture(files: &[&str]) -> (tempfile::TempDir, ContentSources, RagPolicy) {
        use crate::{rag_service::RetrievalMode, settings::SearchScope};
        let directory = tempfile::tempdir().unwrap();
        let folder = directory.path().join("sources");
        fs::create_dir_all(folder.join(".git")).unwrap();
        for name in files {
            let path = folder.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, "guardedpolicycanary shared evidence").unwrap();
        }
        let service = ContentService::open(
            &directory.path().join("index"),
            Err("not installed".into()),
            |_| {},
        )
        .unwrap();
        service
            .configure(vec![SearchScope {
                id: "docs".into(),
                name: "docs".into(),
                path: folder.to_string_lossy().into(),
                filename_enabled: true,
                content_enabled: true,
                external_allowed: false,
            }])
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while service.status().unwrap().phase != "ready" {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        // This fixture tests RAG authority against already-indexed cache data,
        // not a background extraction-policy implementation.
        service.cancel_index();
        let prefs = GenerationPreferences {
            retrieval: RetrievalMode::Keyword,
            ..Default::default()
        };
        let policy = RagPolicy {
            revision: 1,
            prefs,
            scopes: vec![RagScope {
                id: "docs".into(),
                name: "docs".into(),
                path: folder.to_string_lossy().into(),
                content_enabled: true,
                external_allowed: false,
            }],
            file_policy: crate::file_policy::Categories::default(),
            mcp: vec![],
        };
        (directory, ContentSources(service), policy)
    }

    #[test]
    fn persisted_source_identity_is_checked_before_original_hash_reads() {
        let (_directory, resolver, policy) = policy_fixture(&["first.md", "same.md"]);
        let cancel = AtomicBool::new(false);
        let original = resolver
            .retrieve("guardedpolicycanary", &[], &policy, 8, &cancel)
            .unwrap()
            .into_iter()
            .find(|source| source.source.file_path.ends_with("first.md"))
            .unwrap()
            .source;
        let mut unknown = original.clone();
        unknown.chunk_id.push_str("-not-current");
        let mut wrong_hash = original.clone();
        wrong_hash.sha256 = "not-the-current-fingerprint".into();
        let mut wrong_path = original.clone();
        wrong_path.file_path = Path::new(&original.file_path)
            .with_file_name("same.md")
            .to_string_lossy()
            .into_owned();
        // Both files have identical bytes. A hash match cannot substitute for
        // the current chunk's path or ID, and invalid references need no read.
        for source in [unknown, wrong_hash, wrong_path] {
            SOURCE_HASH_REQUESTS.with(|calls| calls.set(0));
            let checked = resolver
                .revalidate(&[source], &[], &policy, &cancel)
                .unwrap();
            assert_eq!(checked[0].state, SourceState::Changed);
            assert!(checked[0].text.is_none());
            assert_eq!(SOURCE_HASH_REQUESTS.with(|calls| calls.get()), 0);
        }
        SOURCE_HASH_REQUESTS.with(|calls| calls.set(0));
        let checked = resolver
            .revalidate(&[original.clone(), original.clone()], &[], &policy, &cancel)
            .unwrap();
        assert!(checked
            .iter()
            .all(|value| value.state == SourceState::Current));
        assert!(checked.iter().all(|value| value.text.is_some()));
        assert_eq!(SOURCE_HASH_REQUESTS.with(|calls| calls.get()), 1);

        // The content service's own exclusions can withdraw a cached chunk
        // even while the broader RAG folder and category policy still allow it.
        resolver
            .0
            .exclude_paths(&[Path::new(&original.file_path).to_path_buf()]);
        assert!(resolver
            .0
            .chunks(std::slice::from_ref(&original.chunk_id), &cancel)
            .unwrap()
            .is_empty());
        SOURCE_HASH_REQUESTS.with(|calls| calls.set(0));
        let checked = resolver
            .revalidate(std::slice::from_ref(&original), &[], &policy, &cancel)
            .unwrap();
        assert_eq!(checked[0].state, SourceState::Changed);
        assert!(checked[0].text.is_none());
        assert_eq!(SOURCE_HASH_REQUESTS.with(|calls| calls.get()), 0);

        fs::remove_file(&original.file_path).unwrap();
        SOURCE_HASH_REQUESTS.with(|calls| calls.set(0));
        let checked = resolver
            .revalidate(&[original], &[], &policy, &cancel)
            .unwrap();
        assert_eq!(checked[0].state, SourceState::Deleted);
        assert!(checked[0].text.is_none());
        assert_eq!(SOURCE_HASH_REQUESTS.with(|calls| calls.get()), 0);
        resolver.0.shutdown();
    }

    #[test]
    fn content_policy_blocks_sha_reads_and_backfills_allowed_lower_ranked_evidence() {
        // More denied matches than the service's 100-hit preview. Filtering
        // only that preview cannot discover the permitted lower-ranked file.
        let mut files: Vec<_> = (0..120).map(|i| format!("a{i:03}.rs")).collect();
        files.push("z-allowed.txt".into());
        let names: Vec<_> = files.iter().map(String::as_str).collect();
        let (_directory, resolver, mut policy) = policy_fixture(&names);
        let cancel = AtomicBool::new(false);
        let before = resolver
            .retrieve("guardedpolicycanary", &[], &policy, 1, &cancel)
            .unwrap();
        assert_eq!(before.len(), 1);
        assert!(before[0].source.file_path.ends_with("a000.rs"));
        policy.file_policy.source_code = false;
        SOURCE_HASH_REQUESTS.with(|calls| calls.set(0));
        let after = resolver
            .retrieve("guardedpolicycanary", &[], &policy, 1, &cancel)
            .unwrap();
        assert_eq!(after.len(), 1);
        assert!(after[0].source.file_path.ends_with("z-allowed.txt"));
        assert_eq!(
            SOURCE_HASH_REQUESTS.with(|calls| calls.get()),
            1,
            "Only admitted evidence may reach the hasher"
        );
        SOURCE_HASH_REQUESTS.with(|calls| calls.set(0));
        let checked = resolver
            .revalidate(&[before[0].source.clone()], &[], &policy, &cancel)
            .unwrap();
        assert_eq!(checked[0].state, SourceState::OutOfScope);
        assert!(checked[0].text.is_none());
        assert_eq!(SOURCE_HASH_REQUESTS.with(|calls| calls.get()), 0);
        assert!(policy.scopes[0].content_enabled && !policy.scopes[0].external_allowed);
        resolver.0.shutdown();
    }

    #[test]
    fn hidden_ignored_and_unverifiable_rules_block_persisted_source_hash_reads() {
        let (directory, resolver, mut policy) =
            policy_fixture(&[".private/secret.md", "ignored.md", "allowed.md"]);
        let cancel = AtomicBool::new(false);
        let originals = resolver
            .retrieve("guardedpolicycanary", &[], &policy, 8, &cancel)
            .unwrap();
        assert_eq!(originals.len(), 3);
        let hidden = originals
            .iter()
            .find(|item| item.source.file_path.contains("secret.md"))
            .unwrap()
            .source
            .clone();
        let ignored = originals
            .iter()
            .find(|item| item.source.file_path.ends_with("ignored.md"))
            .unwrap()
            .source
            .clone();
        policy.file_policy.hidden = false;
        SOURCE_HASH_REQUESTS.with(|calls| calls.set(0));
        assert_eq!(
            resolver
                .revalidate(&[hidden], &[], &policy, &cancel)
                .unwrap()[0]
                .state,
            SourceState::OutOfScope
        );
        assert_eq!(SOURCE_HASH_REQUESTS.with(|calls| calls.get()), 0);
        policy.file_policy.hidden = true;
        policy.file_policy.repository_ignored = false;
        let rules = directory.path().join("sources/.gitignore");
        for bytes in [b"/ignored.md\n".as_slice(), &[0xff, 0xfe]] {
            fs::write(&rules, bytes).unwrap();
            SOURCE_HASH_REQUESTS.with(|calls| calls.set(0));
            let checked = resolver
                .revalidate(std::slice::from_ref(&ignored), &[], &policy, &cancel)
                .unwrap();
            assert_eq!(checked[0].state, SourceState::OutOfScope);
            assert!(checked[0].text.is_none());
            assert_eq!(SOURCE_HASH_REQUESTS.with(|calls| calls.get()), 0);
        }
        resolver.0.shutdown();
    }
    #[test]
    fn document_requirements_do_not_break_keyword_or_unavailable_embedding_fallback() {
        use crate::{rag_service::RetrievalMode, settings::SearchScope};
        let directory = tempfile::tempdir().unwrap();
        let folder = directory.path().join("sources");
        fs::create_dir(&folder).unwrap();
        fs::write(folder.join("source.md"), "shared evidence").unwrap();
        let service = ContentService::open(
            &directory.path().join("index"),
            Err("not installed".into()),
            |_| {},
        )
        .unwrap();
        service
            .configure(vec![SearchScope {
                id: "docs".into(),
                name: "docs".into(),
                path: folder.to_string_lossy().into(),
                filename_enabled: true,
                content_enabled: true,
                external_allowed: false,
            }])
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while service.status().unwrap().phase != "ready" {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let resolver = ContentSources(service);
        let mut policy = RagPolicy {
            file_policy: crate::file_policy::Categories::default(),
            mcp: vec![],
            revision: 1,
            prefs: GenerationPreferences::default(),
            scopes: vec![RagScope {
                id: "docs".into(),
                name: "docs".into(),
                path: folder.to_string_lossy().into(),
                content_enabled: true,
                external_allowed: false,
            }],
        };
        let cancel = AtomicBool::new(false);
        for mode in [RetrievalMode::Keyword, RetrievalMode::Hybrid] {
            policy.prefs.retrieval = mode;
            let found = resolver
                .retrieve_document(
                    DocumentRetrieval {
                        keyword: "shared",
                        semantic: "shared format as a two-column table",
                    },
                    &["docs".into()],
                    &policy,
                    None,
                    8,
                    &cancel,
                )
                .unwrap();
            assert_eq!(found.len(), 1);
            assert!(found[0].text.contains("shared evidence"));
            assert!(resolver
                .retrieve_document(
                    DocumentRetrieval {
                        keyword: "shared",
                        semantic: "shared format as a two-column table"
                    },
                    &["docs".into()],
                    &policy,
                    Some(&[]),
                    8,
                    &cancel,
                )
                .unwrap()
                .is_empty());
        }
    }
    #[cfg(windows)]
    #[test]
    fn real_short_source_alias_preserves_hash_and_chunk_identity_guards() {
        use crate::settings::SearchScope;
        let directory = tempfile::tempdir().unwrap();
        let folder = directory.path().join("A long private source folder");
        fs::create_dir(&folder).unwrap();
        let a = folder.join("source-a.md");
        let b = folder.join("source-b.md");
        fs::write(&a, "shared evidence").unwrap();
        fs::write(&b, "shared evidence").unwrap();
        let Some(short) = crate::path_identity::short_alias(&folder).unwrap() else {
            eprintln!("No short aliases on fixture volume; actual alias case not exercised");
            return;
        };
        let service = ContentService::open(
            &directory.path().join("index"),
            Err("not installed".into()),
            |_| {},
        )
        .unwrap();
        service
            .configure(vec![SearchScope {
                id: "docs".into(),
                name: "docs".into(),
                path: short.to_string_lossy().into(),
                filename_enabled: true,
                content_enabled: true,
                external_allowed: false,
            }])
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while service.status().unwrap().phase != "ready" {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let resolver = ContentSources(service);
        let policy = RagPolicy {
            file_policy: crate::file_policy::Categories::default(),
            mcp: vec![],
            revision: 1,
            prefs: GenerationPreferences::default(),
            scopes: vec![RagScope {
                id: "docs".into(),
                name: "docs".into(),
                path: short.to_string_lossy().into(),
                content_enabled: true,
                external_allowed: false,
            }],
        };
        let cancel = AtomicBool::new(false);
        let original = resolver
            .retrieve("shared", &[], &policy, 8, &cancel)
            .unwrap()
            .into_iter()
            .find(|source| source.source.file_path.ends_with("source-a.md"))
            .unwrap()
            .source;
        let mut alias = original.clone();
        alias.file_path = short.join("source-a.md").to_string_lossy().into();
        let checked = resolver
            .revalidate(&[alias.clone()], &[], &policy, &cancel)
            .unwrap();
        assert_eq!(checked[0].state, SourceState::Current);
        assert!(checked[0].text.as_deref().unwrap().contains("evidence"));
        assert_eq!(
            key(Path::new(&checked[0].source.file_path)),
            key(&fs::canonicalize(&a).unwrap())
        );
        // Same hash at a different path is insufficient for the stored chunk ID.
        alias.file_path = short.join("source-b.md").to_string_lossy().into();
        assert_eq!(
            resolver
                .revalidate(&[alias], &[], &policy, &cancel)
                .unwrap()[0]
                .state,
            SourceState::Changed
        );
        let timestamp = fs::metadata(&a).unwrap().modified().unwrap();
        fs::write(&a, "edited evidence").unwrap();
        File::options()
            .write(true)
            .open(&a)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(timestamp))
            .unwrap();
        assert_eq!(
            resolver
                .revalidate(&[original], &[], &policy, &cancel)
                .unwrap()[0]
                .state,
            SourceState::Changed
        );
        resolver.0.shutdown();
    }
    fn ranking_chunk(id: &str) -> ContentChunk {
        serde_json::from_value(serde_json::json!({
            "id":id,"filePath":format!("/synthetic/{id}.md"),
            "fingerprint":{"sha256":"synthetic","sizeBytes":10,"modifiedUnixMs":1},
            "sectionIndex":0,"chunkIndex":0,"text":"synthetic ranking fixture",
            "page":null,"paragraph":null,"heading":null,"charStart":0,"charEnd":25
        }))
        .unwrap()
    }
    #[test]
    fn retrieval_fuses_global_branch_ranks_instead_of_page_local_ranks() {
        let keyword = |id: &str, score| crate::content_store::ContentSearchHit {
            chunk: ranking_chunk(id),
            score,
            snippet: id.into(),
        };
        let semantic = |id: &str, cosine_distance| crate::vector_store::VectorSearchHit {
            chunk: ranking_chunk(id),
            cosine_distance,
        };
        let mut candidates = RetrievalCandidates::default();
        candidates.keyword.push(keyword("both", 10.0));
        candidates.semantic.push(semantic("both", 0.2));
        candidates.trim(100);
        // A later file page contributes better independent branch matches.
        candidates.keyword.push(keyword("keyword-only", 20.0));
        candidates.semantic.push(semantic("semantic-only", 0.1));
        candidates.trim(100);
        let ranked = candidates.finish(true, true, 3);
        assert_eq!(ranked[0].0.id, "both");
        assert!((ranked[0].1 - 2.0 / 62.0).abs() < 1e-12);
        assert_eq!(ranked.len(), 3);
        assert!((ranked[1].1 - 1.0 / 61.0).abs() < 1e-12);
    }
    #[test]
    fn late_pages_replace_worse_candidates_without_duplicate_or_nonfinite_votes() {
        let mut candidates = RetrievalCandidates::default();
        for (id, distance) in [
            ("old", 0.8),
            ("later", 0.1),
            ("later", 0.1),
            ("bad", f32::NAN),
        ] {
            candidates
                .semantic
                .push(crate::vector_store::VectorSearchHit {
                    chunk: ranking_chunk(id),
                    cosine_distance: distance,
                });
            candidates.trim(1);
        }
        let ranked = candidates.finish(false, true, 1);
        assert_eq!(ranked[0].0.id, "later");
        assert!((ranked[0].1 - 0.9).abs() < 1e-6);
    }
    #[test]
    fn selecting_a_parent_folder_uses_the_most_specific_current_scope() {
        let dir = tempfile::tempdir().unwrap();
        let child = dir.path().join("child");
        fs::create_dir(&child).unwrap();
        let source = child.join("source.md");
        fs::write(&source, "fixture").unwrap();
        let policy = RagPolicy {
            file_policy: crate::file_policy::Categories::default(),
            mcp: vec![],
            revision: 1,
            prefs: GenerationPreferences::default(),
            scopes: vec![
                RagScope {
                    id: "parent".into(),
                    name: "parent".into(),
                    path: dir.path().to_string_lossy().into(),
                    content_enabled: true,
                    external_allowed: true,
                },
                RagScope {
                    id: "child".into(),
                    name: "child".into(),
                    path: child.to_string_lossy().into(),
                    content_enabled: true,
                    external_allowed: false,
                },
            ],
        };
        let folders = selected(&policy, &["parent".into()]).unwrap();
        assert_eq!(
            scope_for(&source, &policy, &folders).as_deref(),
            Some("child")
        );
        let mut duplicate = policy.clone();
        let mut scope = duplicate.scopes[1].clone();
        scope.id = "alias".into();
        duplicate.scopes.push(scope);
        assert!(scope_for(&source, &duplicate, &folders).is_none());
        assert!(selected(&policy, &["removed".into()]).is_err());
    }
    #[test]
    fn hashing_detects_equal_size_edits_even_with_restored_modification_time() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.md");
        fs::write(&source, "before").unwrap();
        let timestamp = fs::metadata(&source).unwrap().modified().unwrap();
        let before = current_sha(&source, &AtomicBool::new(false)).unwrap();
        fs::write(&source, "edited").unwrap();
        File::options()
            .write(true)
            .open(&source)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(timestamp))
            .unwrap();
        assert_ne!(
            current_sha(&source, &AtomicBool::new(false)).unwrap(),
            before
        );
        assert_eq!(
            current_sha(&source, &AtomicBool::new(true))
                .unwrap_err()
                .code,
            RagErrorCode::Cancelled
        );
    }
}
fn reference(chunk: &ContentChunk, scope: String) -> SourceRef {
    SourceRef {
        chunk_id: chunk.id.clone(),
        scope_id: scope,
        file_path: chunk.file_path.clone(),
        sha256: chunk.fingerprint.sha256.clone(),
        page: chunk.page,
        paragraph: chunk.paragraph,
    }
}
/// Keep each branch globally ranked across every saved-selection batch before
/// fusion. The branch budget is per question, never per 256-file page.
#[derive(Default)]
struct RetrievalCandidates {
    keyword: Vec<crate::content_store::ContentSearchHit>,
    semantic: Vec<crate::vector_store::VectorSearchHit>,
}
const HYBRID_BRANCH_LIMIT: usize = 100;
impl RetrievalCandidates {
    fn trim(&mut self, limit: usize) {
        self.keyword.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                // Match ContentStore's global tie order, independent of pages.
                .then_with(|| a.chunk.file_path.cmp(&b.chunk.file_path))
                .then_with(|| a.chunk.section_index.cmp(&b.chunk.section_index))
                .then_with(|| a.chunk.chunk_index.cmp(&b.chunk.chunk_index))
                .then_with(|| a.chunk.id.cmp(&b.chunk.id))
        });
        self.semantic.sort_by(|a, b| {
            a.cosine_distance
                .total_cmp(&b.cosine_distance)
                .then_with(|| a.chunk.id.cmp(&b.chunk.id))
        });
        let mut seen = HashSet::new();
        self.keyword.retain(|hit| seen.insert(hit.chunk.id.clone()));
        seen.clear();
        self.semantic
            .retain(|hit| hit.cosine_distance.is_finite() && seen.insert(hit.chunk.id.clone()));
        self.keyword.truncate(limit);
        self.semantic.truncate(limit);
    }
    fn finish(self, hybrid: bool, semantic: bool, limit: usize) -> Vec<(ContentChunk, f64)> {
        if hybrid {
            crate::hybrid_search::fuse_ranked(self.keyword, self.semantic, limit)
                .into_iter()
                .map(|hit| (hit.chunk, hit.score))
                .collect()
        } else if semantic {
            self.semantic
                .into_iter()
                .take(limit)
                .map(|hit| (hit.chunk, 1.0 - f64::from(hit.cosine_distance)))
                .collect()
        } else {
            self.keyword
                .into_iter()
                .take(limit)
                .map(|hit| (hit.chunk, hit.score))
                .collect()
        }
    }
}
struct RetrievalPlan<'a> {
    question: &'a str,
    ids: &'a [String],
    prepared: Option<&'a crate::content_service::SemanticQuery>,
    hybrid: bool,
    limit: usize,
    policy: &'a RagPolicy,
    folders: &'a [(String, String)],
}
impl ContentSources {
    fn prepare_retrieval(
        &self,
        question: &str,
        mode: crate::rag_service::RetrievalMode,
        cancel: &AtomicBool,
    ) -> RagResult<Option<crate::content_service::SemanticQuery>> {
        if mode == crate::rag_service::RetrievalMode::Keyword {
            return Ok(None);
        }
        match self.0.prepare_semantic_query(question, cancel) {
            Ok(query) => Ok(Some(query)),
            Err(error) if error.code == "model" || error.code == "modelNotRunning" => Ok(None),
            Err(error) => Err(source_error(&error.code)),
        }
    }
    fn collect_retrieval(
        &self,
        plan: &RetrievalPlan<'_>,
        documents: Option<&[String]>,
        candidates: &mut RetrievalCandidates,
        cancel: &AtomicBool,
    ) -> RagResult<()> {
        let RetrievalPlan {
            question,
            ids,
            prepared,
            hybrid,
            limit,
            policy,
            folders,
        } = *plan;
        let mut admit = |path: &Path| {
            policy.file_policy.unrestricted() || scope_for(path, policy, folders).is_some()
        };
        if let Some(query) = prepared {
            let filter = crate::content_store::ContentSearchFilter {
                document_paths: documents.unwrap_or_default().to_vec(),
                ..Default::default()
            };
            candidates.semantic.extend(
                self.0
                    .run_semantic_query_with_admission(
                        query, &filter, ids, limit, cancel, &mut admit,
                    )
                    .map_err(|error| source_error(&error.code))?
                    .hits,
            );
        }
        if hybrid || prepared.is_none() {
            match self
                .0
                .search_selected_with_admission(question, "", ids, documents, cancel, &mut admit)
            {
                Ok(response) => candidates.keyword.extend(response.hits),
                // Long natural-language questions can exceed the lexical term
                // budget. Semantic retrieval remains available in hybrid mode.
                Err(error) if hybrid && error.code.eq_ignore_ascii_case("invalidInput") => {}
                Err(error) => return Err(source_error(&error.code)),
            }
        }
        candidates.trim(limit);
        Ok(())
    }
}
impl SourceResolver for ContentSources {
    fn retrieve_set(
        &self,
        question: &str,
        scope_ids: &[String],
        policy: &RagPolicy,
        selection: Option<&crate::selection_set::SavedSelection>,
        top_k: usize,
        cancel: &AtomicBool,
    ) -> RagResult<Vec<RagSource>> {
        let Some(selection) = selection else {
            return self.retrieve_selected(question, scope_ids, policy, None, top_k, cancel);
        };
        cancel_check(cancel)?;
        let folders = selected(policy, scope_ids)?;
        if folders.is_empty() {
            return Ok(vec![]);
        }
        let ids: Vec<_> = folders.iter().map(|(id, _)| id.clone()).collect();
        let roots: Vec<_> = folders.iter().map(|(_, root)| root.clone()).collect();
        let prepared = self.prepare_retrieval(question, policy.prefs.retrieval, cancel)?;
        let hybrid = policy.prefs.retrieval == crate::rag_service::RetrievalMode::Hybrid
            && prepared.is_some();
        let limit = top_k.clamp(1, 128);
        let branch_limit = if hybrid { HYBRID_BRANCH_LIMIT } else { limit };
        let mut after = String::new();
        let plan = RetrievalPlan {
            question,
            ids: &ids,
            prepared: prepared.as_ref(),
            hybrid,
            limit: branch_limit,
            policy,
            folders: &folders,
        };
        let mut candidates = RetrievalCandidates::default();
        loop {
            cancel_check(cancel)?;
            let batch = selection.batch_in_roots(&after, Some(&roots))?;
            let Some(last) = batch.last() else { break };
            after = crate::chat_selection::path_key(&last.path);
            let paths: Vec<_> = batch.into_iter().map(|file| file.path).collect();
            self.collect_retrieval(&plan, Some(&paths), &mut candidates, cancel)?;
        }
        let best = candidates.finish(hybrid, prepared.is_some(), limit);
        authorize(best, &folders, policy, limit, cancel)
    }
    fn retrieve(
        &self,
        question: &str,
        scope_ids: &[String],
        policy: &RagPolicy,
        top_k: usize,
        cancel: &AtomicBool,
    ) -> RagResult<Vec<RagSource>> {
        self.retrieve_selected(question, scope_ids, policy, None, top_k, cancel)
    }
    fn retrieve_selected(
        &self,
        question: &str,
        scope_ids: &[String],
        policy: &RagPolicy,
        documents: Option<&[String]>,
        top_k: usize,
        cancel: &AtomicBool,
    ) -> RagResult<Vec<RagSource>> {
        self.retrieve_document(
            DocumentRetrieval {
                keyword: question,
                semantic: question,
            },
            scope_ids,
            policy,
            documents,
            top_k,
            cancel,
        )
    }
    fn retrieve_document(
        &self,
        query: DocumentRetrieval<'_>,
        scope_ids: &[String],
        policy: &RagPolicy,
        documents: Option<&[String]>,
        top_k: usize,
        cancel: &AtomicBool,
    ) -> RagResult<Vec<RagSource>> {
        cancel_check(cancel)?;
        if documents.is_some_and(<[String]>::is_empty) {
            return Ok(vec![]);
        }
        let folders = selected(policy, scope_ids)?;
        if folders.is_empty() {
            return Ok(vec![]);
        }
        let ids: Vec<_> = folders.iter().map(|(id, _)| id.clone()).collect();
        let prepared = self.prepare_retrieval(query.semantic, policy.prefs.retrieval, cancel)?;
        let hybrid = policy.prefs.retrieval == crate::rag_service::RetrievalMode::Hybrid
            && prepared.is_some();
        let limit = top_k.clamp(1, 128);
        let branch_limit = if hybrid { HYBRID_BRANCH_LIMIT } else { limit };
        let plan = RetrievalPlan {
            question: query.keyword,
            ids: &ids,
            prepared: prepared.as_ref(),
            hybrid,
            limit: branch_limit,
            policy,
            folders: &folders,
        };
        let mut candidates = RetrievalCandidates::default();
        self.collect_retrieval(&plan, documents, &mut candidates, cancel)?;
        let candidates = candidates.finish(hybrid, prepared.is_some(), limit);
        authorize(candidates, &folders, policy, top_k, cancel)
    }
    fn revalidate(
        &self,
        sources: &[SourceRef],
        scope_ids: &[String],
        policy: &RagPolicy,
        cancel: &AtomicBool,
    ) -> RagResult<Vec<CheckedSource>> {
        self.revalidate_current(sources, scope_ids, policy, cancel)
    }
}
fn source_error(code: &str) -> RagError {
    RagError::new(match code {
        "cancelled" => RagErrorCode::Cancelled,
        "input" | "invalidInput" => RagErrorCode::Context,
        _ => RagErrorCode::SourceChanged,
    })
}
fn authorize(
    mut candidates: Vec<(ContentChunk, f64)>,
    folders: &[(String, String)],
    policy: &RagPolicy,
    top_k: usize,
    cancel: &AtomicBool,
) -> RagResult<Vec<RagSource>> {
    candidates.sort_by(|left, right| right.1.total_cmp(&left.1));
    let mut seen = HashSet::new();
    let mut hashes = HashMap::new();
    let mut result = Vec::new();
    for (chunk, _) in candidates {
        cancel_check(cancel)?;
        if !seen.insert(chunk.id.clone()) {
            continue;
        }
        let path = Path::new(&chunk.file_path);
        let Some(scope) = scope_for(path, policy, folders) else {
            continue;
        };
        let sha = match hashes.entry(chunk.file_path.clone()) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(current_sha(path, cancel)?)
            }
        };
        if sha.as_deref() != Some(chunk.fingerprint.sha256.as_str()) {
            continue;
        }
        result.push(RagSource {
            source: reference(&chunk, scope),
            text: chunk.text,
        });
        if result.len() >= top_k.clamp(1, 128) {
            break;
        }
    }
    Ok(result)
}
impl ContentSources {
    fn revalidate_current(
        &self,
        sources: &[SourceRef],
        scope_ids: &[String],
        policy: &RagPolicy,
        cancel: &AtomicBool,
    ) -> RagResult<Vec<CheckedSource>> {
        cancel_check(cancel)?;
        let folders = selected(policy, scope_ids)?;
        let ids: Vec<_> = sources
            .iter()
            .map(|source| source.chunk_id.clone())
            .collect();
        let mut chunks = HashMap::new();
        for batch in ids.chunks(256) {
            for chunk in self
                .0
                .chunks(batch, cancel)
                .map_err(|_| RagError::new(RagErrorCode::SourceChanged))?
            {
                chunks.insert(chunk.id.clone(), chunk);
            }
        }
        let mut hashes = HashMap::new();
        let mut checked = Vec::new();
        for source in sources {
            cancel_check(cancel)?;
            let path = Path::new(&source.file_path);
            let scope = scope_for(path, policy, &folders);
            let canonical = crate::path_identity::canonical(path).ok();
            let state = if !path.exists() {
                SourceState::Deleted
            } else if scope.is_none() {
                SourceState::OutOfScope
            } else if !chunks.get(&source.chunk_id).is_some_and(|chunk| {
                chunk.fingerprint.sha256 == source.sha256
                    && canonical
                        .as_ref()
                        .is_some_and(|canonical| key(Path::new(&chunk.file_path)) == key(canonical))
            }) {
                // Stored provenance alone never authorizes an original read.
                // The current content service may have withdrawn this chunk.
                SourceState::Changed
            } else {
                let sha = match hashes.entry(source.file_path.clone()) {
                    std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        entry.insert(current_sha(path, cancel)?)
                    }
                };
                if sha.as_deref() == Some(source.sha256.as_str())
                    && crate::path_identity::canonical(path).ok().as_ref() == canonical.as_ref()
                {
                    SourceState::Current
                } else {
                    SourceState::Changed
                }
            };
            let mut reference = source.clone();
            if state == SourceState::Current {
                if let Some(canonical) = canonical {
                    reference.file_path = crate::path_identity::normal(&canonical)
                        .to_string_lossy()
                        .into_owned();
                }
            }
            if let Some(scope) = scope {
                reference.scope_id = scope;
            }
            let text = (state == SourceState::Current)
                .then(|| chunks.get(&source.chunk_id).map(|chunk| chunk.text.clone()))
                .flatten();
            checked.push(CheckedSource {
                source: reference,
                state,
                text,
            });
        }
        Ok(checked)
    }
}

//! Connect conversation evidence to the current local content index. Stored chat
//! provenance never grants file access by itself.
use crate::{
    content_service::ContentService,
    content_store::ContentChunk,
    rag_service::{
        CheckedSource, RagError, RagErrorCode, RagPolicy, RagResult, RagSource, SourceRef,
        SourceResolver, SourceState,
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
            (canonical.is_dir() && within(&path_key, &root))
                .then_some((scope.id.clone(), root.len()))
        })
        .collect();
    scopes.sort_by(|left, right| right.1.cmp(&left.1));
    let first = scopes.first()?;
    if scopes.get(1).is_some_and(|other| other.1 == first.1) {
        return None;
    }
    Some(first.0.clone())
}
// Hash the current authorized file once per preparation pass, even if size and
// modification time were preserved by the editor. No source text is logged.
fn current_sha(path: &Path, cancel: &AtomicBool) -> RagResult<Option<String>> {
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
mod tests {
    use super::*;
    use crate::rag_service::{GenerationPreferences, RagScope};
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
    #[test]
    fn selecting_a_parent_folder_uses_the_most_specific_current_scope() {
        let dir = tempfile::tempdir().unwrap();
        let child = dir.path().join("child");
        fs::create_dir(&child).unwrap();
        let source = child.join("source.md");
        fs::write(&source, "fixture").unwrap();
        let policy = RagPolicy {
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
        let prepared = match self.0.prepare_semantic_query(question, cancel) {
            Ok(value) => Some(value),
            Err(error) if error.code == "model" || error.code == "modelNotRunning" => None,
            Err(error) => return Err(source_error(&error.code)),
        };
        let limit = top_k.clamp(1, 128);
        let mut after = 0;
        let mut best = Vec::new();
        loop {
            cancel_check(cancel)?;
            let batch = selection.batch_in_roots(after, Some(&roots))?;
            let Some(last) = batch.last() else { break };
            after = last.id;
            let paths: Vec<_> = batch.into_iter().map(|file| file.path).collect();
            let candidates: Vec<(ContentChunk, f64)> = if let Some(query) = &prepared {
                let filter = crate::content_store::ContentSearchFilter {
                    document_paths: paths,
                    ..Default::default()
                };
                self.0
                    .run_semantic_query(query, &filter, &ids, limit, cancel)
                    .map_err(|e| source_error(&e.code))?
                    .hits
                    .into_iter()
                    .map(|hit| (hit.chunk, 1.0 - f64::from(hit.cosine_distance)))
                    .collect()
            } else {
                self.0
                    .search_selected(question, "", &ids, Some(&paths), cancel)
                    .map_err(|e| source_error(&e.code))?
                    .hits
                    .into_iter()
                    .map(|hit| (hit.chunk, hit.score))
                    .collect()
            };
            best.extend(candidates);
            best.sort_by(|left: &(ContentChunk, f64), right| {
                right
                    .1
                    .total_cmp(&left.1)
                    .then_with(|| left.0.id.cmp(&right.0.id))
            });
            best.truncate(limit);
        }
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
        cancel_check(cancel)?;
        if documents.is_some_and(<[String]>::is_empty) {
            return Ok(vec![]);
        }
        let folders = selected(policy, scope_ids)?;
        if folders.is_empty() {
            return Ok(vec![]);
        }
        let ids: Vec<_> = folders.iter().map(|(id, _)| id.clone()).collect();
        let candidates = match self.0.semantic_search_selected(
            question,
            "",
            &ids,
            top_k.clamp(1, 128),
            documents,
            cancel,
        ) {
            Ok(response) => response
                .hits
                .into_iter()
                .map(|hit| (hit.chunk, 1.0 - f64::from(hit.cosine_distance)))
                .collect(),
            Err(error) if error.code == "model" || error.code == "modelNotRunning" => self
                .0
                .search_selected(question, "", &ids, documents, cancel)
                .map_err(|_| RagError::new(RagErrorCode::SourceChanged))?
                .hits
                .into_iter()
                .map(|hit| (hit.chunk, hit.score))
                .collect(),
            Err(error) if error.code == "cancelled" => {
                return Err(RagError::new(RagErrorCode::Cancelled))
            }
            Err(error) if error.code == "input" => {
                return Err(RagError::new(RagErrorCode::Context))
            }
            Err(_) => return Err(RagError::new(RagErrorCode::SourceChanged)),
        };
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
            } else {
                let sha = match hashes.entry(source.file_path.clone()) {
                    std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        entry.insert(current_sha(path, cancel)?)
                    }
                };
                if sha.as_deref() == Some(source.sha256.as_str())
                    && chunks.get(&source.chunk_id).is_some_and(|chunk| {
                        chunk.fingerprint.sha256 == source.sha256
                            && canonical.as_ref().is_some_and(|canonical| {
                                key(Path::new(&chunk.file_path)) == key(canonical)
                            })
                            && crate::path_identity::canonical(path).ok().as_ref()
                                == canonical.as_ref()
                    })
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

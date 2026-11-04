//! Combine keyword and cosine result ranks without mixing their incompatible
//! raw scores. Equal-weight reciprocal rank fusion uses k=60 and one vote per
//! chunk per branch. Source authorization/freshness is rechecked by the caller.
use crate::{
    content_store::{ContentChunk, ContentSearchResponse},
    vector_store::VectorSearchResponse,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HybridHit {
    pub chunk: ContentChunk,
    pub score: f64,
    pub snippet: String,
    pub keyword_rank: Option<usize>,
    pub semantic_rank: Option<usize>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HybridResponse {
    pub hits: Vec<HybridHit>,
    /// Sum of branch evaluations, including chunks seen by both branches.
    pub candidate_count: usize,
    pub candidate_limit: usize,
    pub hard_limit_reached: bool,
    pub keyword_candidates: usize,
    pub semantic_candidates: usize,
    pub keyword_skipped: bool,
}

pub fn fuse(
    keyword: Option<ContentSearchResponse>,
    semantic: VectorSearchResponse,
    limit: usize,
) -> HybridResponse {
    let keyword_candidates = keyword.as_ref().map_or(0, |value| value.candidate_count);
    let candidate_limit =
        semantic.candidate_limit + keyword.as_ref().map_or(0, |value| value.candidate_limit);
    let hard_limit_reached = semantic.hard_limit_reached
        || keyword
            .as_ref()
            .is_some_and(|value| value.hard_limit_reached);
    let keyword_skipped = keyword.is_none();
    let mut hits = HashMap::<String, HybridHit>::new();
    let mut conflicts = HashSet::new();
    let mut seen = HashSet::new();
    if let Some(keyword) = keyword {
        for (index, hit) in keyword.hits.into_iter().enumerate() {
            if !seen.insert(hit.chunk.id.clone()) {
                continue;
            }
            hits.insert(
                hit.chunk.id.clone(),
                HybridHit {
                    chunk: hit.chunk,
                    snippet: hit.snippet,
                    score: 1.0 / (60.0 + (index + 1) as f64),
                    keyword_rank: Some(index + 1),
                    semantic_rank: None,
                },
            );
        }
    }
    seen.clear();
    for (index, hit) in semantic.hits.into_iter().enumerate() {
        if !hit.cosine_distance.is_finite() || !seen.insert(hit.chunk.id.clone()) {
            continue;
        }
        let contribution = 1.0 / (60.0 + (index + 1) as f64);
        if let Some(existing) = hits.get_mut(&hit.chunk.id) {
            if existing.chunk.file_path != hit.chunk.file_path
                || existing.chunk.fingerprint != hit.chunk.fingerprint
            {
                conflicts.insert(hit.chunk.id);
                continue;
            }
            existing.score += contribution;
            existing.semantic_rank = Some(index + 1);
        } else {
            hits.insert(
                hit.chunk.id.clone(),
                HybridHit {
                    snippet: hit.chunk.text.chars().take(320).collect(),
                    chunk: hit.chunk,
                    score: contribution,
                    keyword_rank: None,
                    semantic_rank: Some(index + 1),
                },
            );
        }
    }
    let mut hits: Vec<_> = hits
        .into_values()
        .filter(|hit| !conflicts.contains(&hit.chunk.id))
        .collect();
    hits.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.chunk.id.cmp(&right.chunk.id))
    });
    hits.truncate(limit);
    HybridResponse {
        hits,
        candidate_count: keyword_candidates + semantic.candidate_count,
        candidate_limit,
        hard_limit_reached,
        keyword_candidates,
        semantic_candidates: semantic.candidate_count,
        keyword_skipped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn chunk(id: &str) -> ContentChunk {
        serde_json::from_value(serde_json::json!({"id":id,"filePath":"/synthetic.md","fingerprint":{"sha256":"synthetic","sizeBytes":10,"modifiedUnixMs":1},"sectionIndex":0,"chunkIndex":0,"text":"synthetic text","page":null,"paragraph":null,"heading":null,"charStart":0,"charEnd":14})).unwrap()
    }
    fn keyword(ids: &[&str]) -> ContentSearchResponse {
        ContentSearchResponse {
            hits: ids
                .iter()
                .enumerate()
                .map(|(index, id)| crate::content_store::ContentSearchHit {
                    chunk: chunk(id),
                    score: 1e9 - index as f64,
                    snippet: format!("keyword {id}"),
                })
                .collect(),
            candidate_count: 9,
            candidate_limit: 100,
            hard_limit_reached: true,
        }
    }
    fn semantic(ids: &[&str]) -> VectorSearchResponse {
        VectorSearchResponse {
            namespace: "fixture".into(),
            total_vectors: 20,
            hits: ids
                .iter()
                .map(|id| crate::vector_store::VectorSearchHit {
                    chunk: chunk(id),
                    cosine_distance: 0.2,
                })
                .collect(),
            candidate_count: 12,
            candidate_limit: 200,
            filtered_out: 0,
            exhausted: true,
            hard_limit_reached: false,
        }
    }
    #[test]
    fn combines_ranks_keeps_single_branch_matches_and_preserves_diagnostics() {
        let value = fuse(
            Some(keyword(&["keyword-only", "both"])),
            semantic(&["semantic-only", "both"]),
            100,
        );
        assert_eq!(value.hits[0].chunk.id, "both");
        assert_eq!(
            (value.hits[0].keyword_rank, value.hits[0].semantic_rank),
            (Some(2), Some(2))
        );
        assert!((value.hits[0].score - 2.0 / 62.0).abs() < 1e-12);
        assert_eq!(value.hits.len(), 3);
        assert_eq!((value.candidate_count, value.candidate_limit), (21, 300));
        assert!(value.hard_limit_reached);
        assert!(!value.keyword_skipped);
        assert_eq!(value.hits[0].snippet, "keyword both");
    }
    #[test]
    fn duplicate_votes_and_conflicting_source_versions_cannot_boost_a_chunk() {
        let value = fuse(Some(keyword(&["a", "a"])), semantic(&["b", "b"]), 100);
        assert_eq!(value.hits.len(), 2);
        assert_eq!(value.hits[0].score, value.hits[1].score);
        let mut changed = semantic(&["a"]);
        changed.hits[0].chunk.fingerprint.sha256 = "changed".into();
        assert!(fuse(Some(keyword(&["a"])), changed, 100).hits.is_empty());
        let mut changed = semantic(&["a"]);
        changed.hits[0].chunk.fingerprint.modified_unix_ms = Some(2);
        assert!(fuse(Some(keyword(&["a"])), changed, 100).hits.is_empty());
    }
    #[test]
    fn ties_limits_and_missing_keyword_branch_are_explicit() {
        let value = fuse(Some(keyword(&["z"])), semantic(&["a"]), 1);
        assert_eq!(value.hits[0].chunk.id, "a");
        let value = fuse(None, semantic(&["a"]), 20);
        assert!(value.keyword_skipped);
        assert_eq!(value.keyword_candidates, 0);
        assert_eq!(value.hits[0].keyword_rank, None);
        let json = serde_json::to_value(value).unwrap();
        assert_eq!(json["keywordSkipped"], true);
        assert_eq!(json["hits"][0]["semanticRank"], 1);
    }
}

//! Result metadata for automatic fallback after filename search finds nothing.
//! It never installs or starts a model. Failed semantic work is visible even
//! when current authorized keyword results remain usable.
use crate::{
    content_store::ContentSearchResponse,
    hybrid_search::{HybridHit, HybridResponse},
    vector_store::VectorSearchResponse,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AutomaticRanking {
    Keyword,
    Cosine,
    Hybrid,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SemanticState {
    Disabled,
    NotRunning,
    Used,
    Failed,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomaticResponse {
    pub hits: Vec<HybridHit>,
    pub candidate_count: usize,
    pub candidate_limit: usize,
    pub hard_limit_reached: bool,
    pub ranking: AutomaticRanking,
    pub semantic_state: SemanticState,
    /// Stable error code only, never a backend message or source text.
    pub semantic_failure: Option<String>,
    pub keyword_skipped: bool,
}
impl AutomaticResponse {
    pub fn keyword(
        value: ContentSearchResponse,
        state: SemanticState,
        failure: Option<String>,
    ) -> Self {
        Self {
            hits: value
                .hits
                .into_iter()
                .enumerate()
                .map(|(index, hit)| HybridHit {
                    chunk: hit.chunk,
                    score: hit.score,
                    snippet: hit.snippet,
                    keyword_rank: Some(index + 1),
                    semantic_rank: None,
                })
                .collect(),
            candidate_count: value.candidate_count,
            candidate_limit: value.candidate_limit,
            hard_limit_reached: value.hard_limit_reached,
            ranking: AutomaticRanking::Keyword,
            semantic_state: state,
            semantic_failure: failure,
            keyword_skipped: false,
        }
    }
    pub fn semantic(value: VectorSearchResponse) -> Self {
        Self {
            hits: value
                .hits
                .into_iter()
                .enumerate()
                .map(|(index, hit)| HybridHit {
                    snippet: hit.chunk.text.chars().take(320).collect(),
                    chunk: hit.chunk,
                    score: 1.0 - f64::from(hit.cosine_distance),
                    keyword_rank: None,
                    semantic_rank: Some(index + 1),
                })
                .collect(),
            candidate_count: value.candidate_count,
            candidate_limit: value.candidate_limit,
            hard_limit_reached: value.hard_limit_reached,
            ranking: AutomaticRanking::Cosine,
            semantic_state: SemanticState::Used,
            semantic_failure: None,
            keyword_skipped: false,
        }
    }
    pub fn hybrid(value: HybridResponse) -> Self {
        Self {
            hits: value.hits,
            candidate_count: value.candidate_count,
            candidate_limit: value.candidate_limit,
            hard_limit_reached: value.hard_limit_reached,
            ranking: AutomaticRanking::Hybrid,
            semantic_state: SemanticState::Used,
            semantic_failure: None,
            keyword_skipped: value.keyword_skipped,
        }
    }
}

pub fn can_fallback_after_semantic_error(code: &str) -> bool {
    !["cancelled", "input", "invalidInput"]
        .iter()
        .any(|terminal| code.eq_ignore_ascii_case(terminal))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancellation_and_input_errors_cannot_become_successful_fallbacks() {
        for code in ["cancelled", "Cancelled", "input", "invalidInput"] {
            assert!(!can_fallback_after_semantic_error(code));
        }
        for code in ["model", "modelNotRunning", "vector", "source", "worker"] {
            assert!(can_fallback_after_semantic_error(code));
        }
        let value = AutomaticResponse::keyword(
            ContentSearchResponse {
                hits: vec![],
                candidate_count: 0,
                candidate_limit: 100,
                hard_limit_reached: false,
            },
            SemanticState::Failed,
            Some("vector".into()),
        );
        let value = serde_json::to_value(value).unwrap();
        assert_eq!(value["semanticState"], "failed");
        assert_eq!(value["semanticFailure"], "vector");
        assert_eq!(value["ranking"], "keyword");
    }
}

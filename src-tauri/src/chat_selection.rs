//! A saved search result set is a restriction, never a grant of file access.
use crate::rag_service::{RagError, RagErrorCode, RagResult};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatSelection {
    pub query: String,
    pub documents: Vec<String>,
    pub selected_documents: Vec<String>,
    pub partial: bool,
}
pub fn path_key(path: &str) -> String {
    let path = path.replace('\\', "/");
    let path = if let Some(tail) = path.strip_prefix("//?/UNC/") {
        format!("//{tail}")
    } else {
        path.strip_prefix("//?/").unwrap_or(&path).to_owned()
    };
    #[cfg(windows)]
    let path = path.to_lowercase();
    path
}
pub fn includes(paths: Option<&[String]>, file: &str) -> bool {
    paths.is_none_or(|paths| paths.iter().any(|path| path_key(path) == path_key(file)))
}
pub fn validate(selection: &ChatSelection) -> RagResult<()> {
    let bad = || RagError::new(RagErrorCode::Input);
    if selection.query.len() > 4096
        || selection.query.contains('\0')
        || selection.documents.len() > 256
        || selection.selected_documents.len() > 256
        || selection.documents.iter().map(String::len).sum::<usize>() > 256 * 1024
    {
        return Err(bad());
    }
    let mut seen = HashSet::new();
    for path in &selection.documents {
        if path.is_empty()
            || path.len() > 32768
            || path.chars().any(char::is_control)
            || !seen.insert(path_key(path))
        {
            return Err(bad());
        }
    }
    let mut chosen = HashSet::new();
    for path in &selection.selected_documents {
        if !seen.contains(&path_key(path)) || !chosen.insert(path_key(path)) {
            return Err(bad());
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_or_empty_selection_never_means_all_documents() {
        assert!(!includes(Some(&[]), "source.md"));
        assert!(includes(None, "source.md"));
        let mut selection = ChatSelection {
            query: "query".into(),
            documents: vec!["source.md".into()],
            selected_documents: vec!["other.md".into()],
            partial: false,
        };
        assert!(validate(&selection).is_err());
        selection.selected_documents.clear();
        assert!(validate(&selection).is_ok());
        selection.documents.push("source.md".into());
        assert!(validate(&selection).is_err());
        selection.documents = vec!["bad\npath".into()];
        assert!(validate(&selection).is_err());
        selection.documents = (0..257).map(|i| format!("file{i}")).collect();
        assert!(validate(&selection).is_err());
    }
}

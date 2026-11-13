//! Trusted document decoder constraints, independent of storage and providers.
use serde_json::Value;
const MAX_DOCUMENT: usize = 128 * 1024;

/// The grammar assists a compatible local decoder. Backend validation below
/// remains authoritative, including byte budgets, unique IDs and source checks.
pub(crate) fn response_schema(
    outline: bool,
    section_ids: &[&str],
    rewrite_targets: Option<&[String]>,
) -> Value {
    let id = serde_json::json!({
        "type":"string","minLength":1,"maxLength":96,"pattern":"^[A-Za-z0-9_-]+$"
    });
    if outline {
        return serde_json::json!({
            "type":"object","additionalProperties":false,"required":["title","sections"],
            "properties":{
                "title":{"type":"string","minLength":1,"maxLength":200},
                "sections":{"type":"array","minItems":1,"maxItems":64,"items":{
                    "type":"object","additionalProperties":false,
                    "required":["id","heading","instruction"],
                    "properties":{
                        "id":id,
                        "heading":{"type":"string","minLength":1,"maxLength":300},
                        "instruction":{"type":"string","maxLength":2000}
                    }
                }}
            }
        });
    }
    let mut sections: Vec<Value> = section_ids
        .iter()
        .map(|id| Value::String((*id).to_owned()))
        .collect();
    sections.push(Value::Null);
    let block_id = if let Some(targets) = rewrite_targets {
        serde_json::json!({"type":"string","enum":targets})
    } else {
        id
    };
    // Each body section needs an assigned block; intros can add more. This
    // count alone cannot establish coverage, so the backend still checks IDs.
    // A partial rewrite need not cover the whole document's reviewed plan.
    let minimum_blocks = if rewrite_targets.is_some() {
        1
    } else {
        section_ids.len().max(1)
    };
    serde_json::json!({
        "type":"object","additionalProperties":false,"required":["blocks"],
        "properties":{"blocks":{"type":"array","minItems":minimum_blocks,"maxItems":256,"items":{
            "type":"object","additionalProperties":false,
            "required":["id","sectionId","kind","markdown","sourceLabels"],
            "properties":{
                "id":block_id,
                "sectionId":{"enum":sections},
                "kind":{"type":"string","enum":["fact","userInput","aiProposal","blank"]},
                "markdown":{"type":"string","maxLength":MAX_DOCUMENT},
                "sourceLabels":{"type":"array","maxItems":32,"items":{
                    "type":"string","minLength":4,"maxLength":12,"pattern":"^\\[[SE][0-9]+\\]$"
                }}
            }
        }}}
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reviewed_ids_are_literal_enums_and_backend_owned_fields_are_excluded() {
        let body = response_schema(false, &["reviewed_a", "reviewed_b"], None);
        assert_eq!(body["properties"]["blocks"]["minItems"], 2);
        let block = &body["properties"]["blocks"]["items"];
        assert_eq!(
            block["properties"]["sectionId"]["enum"],
            serde_json::json!(["reviewed_a", "reviewed_b", null])
        );
        assert!(block["properties"].get("claimsVerified").is_none());
        assert!(block["properties"].get("userEdited").is_none());
        assert_eq!(block["additionalProperties"], false);
        let rewritten = response_schema(
            false,
            &["reviewed_a", "reviewed_b"],
            Some(&["original_block".into()]),
        );
        assert_eq!(rewritten["properties"]["blocks"]["minItems"], 1);
        assert_eq!(
            rewritten["properties"]["blocks"]["items"]["properties"]["id"]["enum"],
            serde_json::json!(["original_block"])
        );
    }

    #[test]
    fn maximum_reviewed_plan_and_rewrite_fit_the_transport_schema_budget() {
        let sections: Vec<_> = (0..64).map(|n| format!("{n:096}")).collect();
        let refs: Vec<_> = sections.iter().map(String::as_str).collect();
        let targets: Vec<_> = (0..256).map(|n| format!("{n:096}")).collect();
        for schema in [
            response_schema(true, &[], None),
            response_schema(false, &refs, Some(&targets)),
        ] {
            assert!(serde_json::to_vec(&schema).unwrap().len() < 64 * 1024);
        }
    }
}

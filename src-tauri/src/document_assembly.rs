//! The optional, reviewed layout for source-backed field extraction.
//! Stored plans describe presentation, never expected evidence values.
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssemblyPlan {
    pub version: u8,
    pub notice_language: NoticeLanguage,
    pub fields: Vec<AssemblyField>,
    pub sections: Vec<AssemblySection>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum NoticeLanguage {
    Ko,
    En,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssemblyField {
    pub id: String,
    pub label: String,
    pub instruction: String,
    pub missing_display: MissingDisplay,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MissingDisplay {
    Blank,
    Notice,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssemblySection {
    pub section_id: String,
    pub elements: Vec<AssemblyElement>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum AssemblyElement {
    Field {
        field_id: String,
    },
    FieldTable {
        columns: [String; 2],
        fields: Vec<String>,
    },
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 96
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn short_text(value: &str, max_chars: usize, max_bytes: usize) -> bool {
    value.len() <= max_bytes
        && value.chars().count() <= max_chars
        && !value.chars().any(char::is_control)
}

impl AssemblyPlan {
    /// Preserve section order and reject dangling, hidden or excessive fields.
    /// Decoder constraints and UI checks never replace this stored-plan check.
    pub(crate) fn validate(&self, section_ids: &[&str]) -> Result<(), &'static str> {
        if self.version != 1
            || self.fields.is_empty()
            || self.fields.len() > 64
            || section_ids.is_empty()
            || section_ids.len() > 64
            || self.sections.len() != section_ids.len()
            || serde_json::to_vec(self).map_or(true, |bytes| bytes.len() > 32 * 1024)
        {
            return Err("Invalid assembly budget or version");
        }
        let mut declared = HashSet::new();
        for field in &self.fields {
            if !identifier(&field.id)
                || !declared.insert(field.id.as_str())
                || field.label.trim().is_empty()
                || !short_text(&field.label, 128, 512)
                || !short_text(&field.instruction, 512, 2_048)
            {
                return Err("Invalid assembly field");
            }
        }
        let mut sections = HashSet::new();
        let mut used = HashSet::new();
        let mut elements = 0usize;
        let mut references = 0usize;
        for (section, expected) in self.sections.iter().zip(section_ids) {
            if section.section_id != *expected
                || !identifier(expected)
                || !sections.insert(*expected)
                || section.elements.is_empty()
            {
                return Err("Assembly sections must match the reviewed outline");
            }
            elements += section.elements.len();
            for element in &section.elements {
                let fields: &[String] = match element {
                    AssemblyElement::Field { field_id } => std::slice::from_ref(field_id),
                    AssemblyElement::FieldTable { columns, fields } => {
                        if fields.is_empty()
                            || columns[0] == columns[1]
                            || columns.iter().any(|column| {
                                column.trim().is_empty() || !short_text(column, 128, 512)
                            })
                        {
                            return Err("Invalid field table");
                        }
                        fields
                    }
                };
                references += fields.len();
                for field in fields {
                    if !declared.contains(field.as_str()) {
                        return Err("Unknown assembly field reference");
                    }
                    used.insert(field.as_str());
                }
            }
        }
        if elements > 128 || references > 256 || used != declared {
            return Err("Unused fields or excessive assembly layout");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan() -> AssemblyPlan {
        serde_json::from_value(serde_json::json!({
            "version":1,"noticeLanguage":"en",
            "fields":[
                {"id":"cost","label":"Cost","instruction":"Preserve the recorded currency.","missingDisplay":"blank"},
                {"id":"reference","label":"Reference","instruction":"Extract the assigned identifier only.","missingDisplay":"notice"}
            ],
            "sections":[{"sectionId":"details","elements":[
                {"kind":"fieldTable","columns":["Field","Value"],"fields":["cost"]},
                {"kind":"field","fieldId":"reference"}
            ]}]
        }))
        .unwrap()
    }

    #[test]
    fn reviewed_plan_round_trips_without_expected_fact_values() {
        let value = plan();
        value.validate(&["details"]).unwrap();
        let json = serde_json::to_string(&value).unwrap();
        assert_eq!(serde_json::from_str::<AssemblyPlan>(&json).unwrap(), value);
        assert!(!json.contains("sourceLabels"));
        assert!(!json.contains("claimsVerified"));
    }

    #[test]
    fn outline_reordering_removal_and_duplicate_sections_need_a_new_layout() {
        for ids in [vec![], vec!["other"], vec!["details", "other"]] {
            assert!(plan().validate(&ids).is_err());
        }
        let mut duplicate = plan();
        duplicate.sections.push(duplicate.sections[0].clone());
        assert!(duplicate.validate(&["details", "details"]).is_err());
    }

    #[test]
    fn reject_dangling_unused_and_duplicate_fields() {
        let mut value = plan();
        value.fields.push(value.fields[0].clone());
        assert!(value.validate(&["details"]).is_err());
        let mut value = plan();
        value.fields[1].id = "unused".into();
        assert!(value.validate(&["details"]).is_err());
        let mut value = plan();
        value.sections[0].elements.pop();
        assert!(value.validate(&["details"]).is_err());
    }

    #[test]
    fn reject_unknown_versions_controls_and_unbounded_layouts() {
        let mut value = plan();
        value.version = 2;
        assert!(value.validate(&["details"]).is_err());
        let mut value = plan();
        value.fields[0].label = "First\nSecond".into();
        assert!(value.validate(&["details"]).is_err());
        let mut value = plan();
        value.fields[0].instruction = "x".repeat(2_049);
        assert!(value.validate(&["details"]).is_err());
        let mut value = plan();
        value.sections[0].elements = vec![value.sections[0].elements[1].clone(); 257];
        assert!(value.validate(&["details"]).is_err());
    }

    #[test]
    fn repeated_reference_sites_are_allowed_but_no_extra_contract_fields() {
        let mut value = plan();
        let repeated = value.sections[0].elements[1].clone();
        value.sections[0].elements.push(repeated);
        value.validate(&["details"]).unwrap();
        let mut json = serde_json::to_value(value).unwrap();
        json["fields"][0]["expectedValue"] = "a hidden fixture answer".into();
        assert!(serde_json::from_value::<AssemblyPlan>(json).is_err());
    }
}

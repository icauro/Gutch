//! Provider-neutral references to tags used by remote provider rulesets.

use serde::Serialize;

#[derive(Debug, Clone)]
pub struct TagReference {
    pub tag_name: String,
    pub tag_kind: String,
    pub referenced_values: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TagReferenceStatus {
    pub tag_name: String,
    pub tag_kind: String,
    pub referenced_values: Vec<String>,
    pub status: String,
}

/// Converts remote rule references into display-only advisory rows.
pub fn advisory_tag_references(references: &[TagReference]) -> Vec<TagReferenceStatus> {
    let mut statuses = references
        .iter()
        .map(|reference| TagReferenceStatus {
            tag_name: reference.tag_name.clone(),
            tag_kind: reference.tag_kind.clone(),
            referenced_values: reference.referenced_values.clone(),
            status: "advisory".to_string(),
        })
        .collect::<Vec<_>>();
    statuses.sort_by(|left, right| left.tag_name.cmp(&right.tag_name));
    statuses
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references_are_always_advisory_and_sorted() {
        let statuses = advisory_tag_references(&[
            TagReference {
                tag_name: "tier".to_string(),
                tag_kind: "key_value".to_string(),
                referenced_values: vec!["supporter".to_string()],
            },
            TagReference {
                tag_name: "os".to_string(),
                tag_kind: "key_value".to_string(),
                referenced_values: vec![],
            },
        ]);

        assert_eq!(statuses[0].tag_name, "os");
        assert!(statuses.iter().all(|status| status.status == "advisory"));
    }
}

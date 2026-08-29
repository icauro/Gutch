//! Provider-neutral comparison of remote tag requirements with a local schema.

use std::collections::HashSet;

use serde::Serialize;

use crate::manifest::{InferenceHandler, SnapshotTagKind, SnapshotTagSchema};

#[derive(Debug, Clone)]
pub struct TagRequirement {
    pub tag_name: String,
    pub tag_kind: String,
    pub allowed_values: Vec<String>,
    pub inference_strategy: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TagRequirementStatus {
    pub tag_name: String,
    pub tag_kind: String,
    pub allowed_values: Vec<String>,
    pub inference_strategy: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issue: Option<String>,
}

pub fn compare_tag_requirements(
    schema: Option<&SnapshotTagSchema>,
    requirements: &[TagRequirement],
) -> Vec<TagRequirementStatus> {
    let mut statuses = requirements
        .iter()
        .map(|requirement| compare_tag_requirement(schema, requirement))
        .collect::<Vec<_>>();

    if !requirements
        .iter()
        .any(|requirement| requirement.tag_name == "version")
    {
        statuses.push(compare_tag_requirement(
            schema,
            &TagRequirement {
                tag_name: "version".to_string(),
                tag_kind: "exclusive".to_string(),
                allowed_values: Vec::new(),
                inference_strategy: "version".to_string(),
            },
        ));
    }

    statuses.sort_by(|left, right| left.tag_name.cmp(&right.tag_name));
    statuses
}

fn compare_tag_requirement(
    schema: Option<&SnapshotTagSchema>,
    requirement: &TagRequirement,
) -> TagRequirementStatus {
    let mut status = TagRequirementStatus {
        tag_name: requirement.tag_name.clone(),
        tag_kind: requirement.tag_kind.clone(),
        allowed_values: requirement.allowed_values.clone(),
        inference_strategy: requirement.inference_strategy.clone(),
        status: "missing".to_string(),
        issue: Some("Add this tag to the local Gutch schema.".to_string()),
    };

    let Some(definition) = schema.and_then(|schema| schema.find_definition(&requirement.tag_name))
    else {
        return status;
    };

    let local_kind = match definition.kind {
        SnapshotTagKind::Plain => "plain",
        SnapshotTagKind::Scoped => "scoped",
        SnapshotTagKind::Exclusive => "exclusive",
    };
    if local_kind != requirement.tag_kind {
        status.status = "incompatible".to_string();
        status.issue = Some(format!("Local tag kind is '{}'.", local_kind));
        return status;
    }

    let local_inference_strategy = match definition.inference_handler {
        InferenceHandler::Default => "default",
        InferenceHandler::Version => "version",
        InferenceHandler::Date => "date",
    };
    if local_inference_strategy != requirement.inference_strategy {
        status.status = "incompatible".to_string();
        status.issue = Some(format!(
            "Local inference strategy is '{}'.",
            local_inference_strategy
        ));
        return status;
    }

    let local_values = definition
        .values
        .iter()
        .map(|value| value.value.as_str())
        .collect::<HashSet<_>>();
    let missing_values = requirement
        .allowed_values
        .iter()
        .filter(|value| !local_values.contains(value.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    if !missing_values.is_empty() {
        status.status = "incompatible".to_string();
        status.issue = Some(format!(
            "Local schema does not allow: {}.",
            missing_values.join(", ")
        ));
        return status;
    }

    status.status = "ready".to_string();
    status.issue = None;
    status
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{SnapshotTagDefinition, SnapshotTagKind};

    fn schema() -> SnapshotTagSchema {
        SnapshotTagSchema {
            snapshot_tags: vec![SnapshotTagDefinition {
                name: "version".to_string(),
                kind: SnapshotTagKind::Exclusive,
                required: true,
                inference_prefixes: vec!["-".to_string()],
                values: vec![],
                default_value: None,
                inference_handler: InferenceHandler::Version,
            }],
        }
    }

    #[test]
    fn comparison_adds_the_required_version_rule() {
        let statuses = compare_tag_requirements(Some(&schema()), &[]);
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].tag_name, "version");
        assert_eq!(statuses[0].status, "ready");
    }

    #[test]
    fn comparison_reports_kind_and_value_mismatches() {
        let statuses = compare_tag_requirements(
            Some(&schema()),
            &[TagRequirement {
                tag_name: "version".to_string(),
                tag_kind: "plain".to_string(),
                allowed_values: vec!["1.0".to_string()],
                inference_strategy: "default".to_string(),
            }],
        );
        assert_eq!(statuses[0].status, "incompatible");
        assert_eq!(
            statuses[0].issue.as_deref(),
            Some("Local tag kind is 'exclusive'.")
        );
    }
}

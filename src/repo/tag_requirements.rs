//! Provider-neutral comparison of remote tag requirements with a local schema.

use serde::Serialize;

use crate::manifest::{
    InferenceHandler, SnapshotTagKind, SnapshotTagSchema, is_valid_snapshot_tag_name,
};

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
    _schema: Option<&SnapshotTagSchema>,
    requirements: &[TagRequirement],
) -> Vec<TagRequirementStatus> {
    let mut statuses = requirements
        .iter()
        .map(|requirement| TagRequirementStatus {
            tag_name: requirement.tag_name.clone(),
            tag_kind: requirement.tag_kind.clone(),
            allowed_values: requirement.allowed_values.clone(),
            inference_strategy: String::new(),
            status: "advisory".to_string(),
            issue: None,
        })
        .collect::<Vec<_>>();
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

    if !is_valid_snapshot_tag_name(&requirement.tag_name) {
        status.status = "incompatible".to_string();
        status.issue = Some("Remote tag name is not lower-snake-case.".to_string());
        return status;
    }

    let expected_kind = match requirement.tag_kind.as_str() {
        "scoped" => SnapshotTagKind::Scoped,
        "exclusive" => SnapshotTagKind::Exclusive,
        _ => {
            status.status = "incompatible".to_string();
            status.issue = Some("Remote tag kind is unsupported.".to_string());
            return status;
        }
    };
    if requirement.tag_name == "version" && expected_kind != SnapshotTagKind::Exclusive {
        status.status = "incompatible".to_string();
        status.issue = Some("The version tag must be exclusive.".to_string());
        return status;
    }

    let Some(definition) = schema.and_then(|schema| schema.find_definition(&requirement.tag_name))
    else {
        return status;
    };

    let local_kind = match definition.kind {
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

    let missing_values = requirement
        .allowed_values
        .iter()
        .filter(|value| {
            !definition
                .values
                .iter()
                .any(|local| local.matches_token(value))
        })
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
    fn comparison_does_not_add_a_version_rule() {
        let statuses = compare_tag_requirements(Some(&schema()), &[]);
        assert!(statuses.is_empty());
    }

    #[test]
    fn comparison_reports_kind_and_value_mismatches() {
        let statuses = compare_tag_requirements(
            Some(&schema()),
            &[TagRequirement {
                tag_name: "version".to_string(),
                tag_kind: "scoped".to_string(),
                allowed_values: vec!["1.0".to_string()],
                inference_strategy: "default".to_string(),
            }],
        );
        assert_eq!(statuses[0].status, "advisory");
        assert!(statuses[0].issue.is_none());
    }

    #[test]
    fn comparison_accepts_case_insensitive_aliases() {
        let mut tag_schema = schema();
        tag_schema.snapshot_tags.push(SnapshotTagDefinition {
            name: "platform".to_string(),
            kind: SnapshotTagKind::Exclusive,
            required: true,
            inference_prefixes: vec![],
            values: vec![crate::manifest::SnapshotTagValueDefinition {
                value: "windows".to_string(),
                aliases: vec!["win".to_string()],
            }],
            default_value: None,
            inference_handler: InferenceHandler::Default,
        });

        let statuses = compare_tag_requirements(
            Some(&tag_schema),
            &[TagRequirement {
                tag_name: "platform".to_string(),
                tag_kind: "exclusive".to_string(),
                allowed_values: vec!["WIN".to_string()],
                inference_strategy: "default".to_string(),
            }],
        );

        assert_eq!(
            statuses
                .iter()
                .find(|status| status.tag_name == "platform")
                .map(|status| status.status.as_str()),
            Some("advisory")
        );
    }
}

use std::collections::HashSet;
use std::path::PathBuf;

use serde::Serialize;

use crate::remote::backend::{
    CreatorAccessTagRequirement, fetch_creator_access_tag_requirements, fetch_creator_repo_by_id,
};
use crate::{Error, Result};

#[derive(Debug, Clone, Serialize)]
pub struct RepoTagRequirementStatus {
    pub tag_name: String,
    pub tag_kind: String,
    pub allowed_values: Vec<String>,
    pub inference_strategy: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issue: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RepoTagRequirementsResult {
    pub repo_root: String,
    pub product_id: i64,
    pub remote_repo_id: i64,
    pub ruleset_id: Option<i64>,
    pub ruleset_version: Option<i32>,
    pub required_tags: Vec<RepoTagRequirementStatus>,
    pub ready: bool,
}

pub fn inspect_repo_tag_requirements(
    repo: PathBuf,
    endpoint: String,
    session_id: String,
) -> Result<RepoTagRequirementsResult> {
    let (repo_paths, repo_manifest) = crate::load_gutch_repo_manifest(&repo)?;
    let remote_repo_id = repo_manifest.remote_repo_id_value().ok_or_else(|| {
        Error::manifest(format!(
            "repo '{}' is not linked. Use 'gutch repo link' first.",
            repo_paths.repo_root.display()
        ))
    })?;
    let client = reqwest::blocking::Client::builder()
        .user_agent("Gutch CLI")
        .build()
        .map_err(|err| {
            Error::manifest(format!("failed to create tag requirements client: {}", err))
        })?;
    let remote_repo = fetch_creator_repo_by_id(&client, &endpoint, &session_id, remote_repo_id)?;
    if remote_repo.product_id <= 0 {
        return Err(Error::manifest(format!(
            "backend response for remote repo {} is missing product_id",
            remote_repo_id
        )));
    }
    let backend_result = fetch_creator_access_tag_requirements(
        &client,
        &endpoint,
        &session_id,
        remote_repo.product_id,
    )?;
    let mut required_tags = backend_result
        .required_tags
        .into_iter()
        .map(|requirement| {
            compare_requirement(repo_manifest.snapshot_tag_schema.as_ref(), requirement)
        })
        .collect::<Vec<_>>();

    if !required_tags.iter().any(|tag| tag.tag_name == "version") {
        required_tags.push(compare_local_requirement(
            repo_manifest.snapshot_tag_schema.as_ref(),
            "version",
            "exclusive",
            &[],
            "version",
        ));
    }
    required_tags.sort_by(|left, right| left.tag_name.cmp(&right.tag_name));

    Ok(RepoTagRequirementsResult {
        repo_root: repo_paths.repo_root.display().to_string(),
        product_id: remote_repo.product_id,
        remote_repo_id,
        ruleset_id: backend_result.ruleset_id,
        ruleset_version: backend_result.ruleset_version,
        ready: required_tags.iter().all(|tag| tag.status == "ready"),
        required_tags,
    })
}

fn compare_requirement(
    schema: Option<&crate::manifest::SnapshotTagSchema>,
    requirement: CreatorAccessTagRequirement,
) -> RepoTagRequirementStatus {
    compare_local_requirement(
        schema,
        &requirement.tag_name,
        &requirement.tag_kind,
        &requirement.allowed_values,
        &requirement.inference_strategy,
    )
}

fn compare_local_requirement(
    schema: Option<&crate::manifest::SnapshotTagSchema>,
    tag_name: &str,
    required_kind: &str,
    allowed_values: &[String],
    required_inference_strategy: &str,
) -> RepoTagRequirementStatus {
    let mut status = RepoTagRequirementStatus {
        tag_name: tag_name.to_string(),
        tag_kind: required_kind.to_string(),
        allowed_values: allowed_values.to_vec(),
        inference_strategy: required_inference_strategy.to_string(),
        status: "missing".to_string(),
        issue: Some("Add this tag to the local Gutch schema.".to_string()),
    };

    let Some(definition) = schema.and_then(|schema| schema.find_definition(tag_name)) else {
        return status;
    };

    let local_kind = match definition.kind {
        crate::manifest::SnapshotTagKind::Plain => "plain",
        crate::manifest::SnapshotTagKind::Scoped => "scoped",
        crate::manifest::SnapshotTagKind::Exclusive => "exclusive",
    };
    if local_kind != required_kind {
        status.status = "incompatible".to_string();
        status.issue = Some(format!("Local tag kind is '{}'.", local_kind));
        return status;
    }

    let local_inference_strategy = match definition.inference_handler {
        crate::manifest::InferenceHandler::Default => "default",
        crate::manifest::InferenceHandler::Version => "version",
        crate::manifest::InferenceHandler::Date => "date",
    };
    if local_inference_strategy != required_inference_strategy {
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
    let missing_values = allowed_values
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
    use super::compare_local_requirement;
    use crate::manifest::{SnapshotTagDefinition, SnapshotTagKind, SnapshotTagSchema};

    fn schema() -> SnapshotTagSchema {
        SnapshotTagSchema {
            snapshot_tags: vec![SnapshotTagDefinition {
                name: "version".to_string(),
                kind: SnapshotTagKind::Exclusive,
                required: true,
                inference_prefixes: vec!["-".to_string()],
                values: vec![],
                default_value: None,
                inference_handler: crate::manifest::InferenceHandler::Version,
            }],
        }
    }

    #[test]
    fn version_requirement_is_ready_when_local_schema_matches() {
        let result =
            compare_local_requirement(Some(&schema()), "version", "exclusive", &[], "version");
        assert_eq!(result.status, "ready");
        assert!(result.issue.is_none());
    }
}

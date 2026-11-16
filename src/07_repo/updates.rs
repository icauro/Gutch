//! Repository rename and constraint updates, with tag validation.

use std::path::Path;
use std::path::PathBuf;

use crate::Result;
use crate::filesystem;
use crate::manifest::SnapshotTagSet;
use crate::manifest::constraints::RepoConstraints;
use crate::progress::OperationControl;
use crate::repo::coordination;
use crate::repo::paths::load_repo_manifest;
use crate::repo::paths::write_repo_manifest;
use serde::Serialize;
use std::collections::BTreeMap;
use std::collections::BTreeSet;

/// The complete constraints that replace a repository's current ones.
#[derive(Debug, Clone, Default)]
pub struct RepoConstraintsSetRequest {
    pub repo_root: PathBuf,
    pub constraints: RepoConstraints,
    pub control: OperationControl,
}

/// The normalized constraints a replacement would store.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RepoConstraintsSetPlan {
    pub constraints: RepoConstraints,
}

/// The normalized constraints that were stored.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RepoConstraintsSetResult {
    pub constraints: RepoConstraints,
}

/// The new display name for a repository.
#[derive(Debug, Clone, Default)]
pub struct RepoRenameRequest {
    pub repo_root: PathBuf,
    pub name: String,
    pub control: OperationControl,
}

/// The validated name a rename would store.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RepoRenamePlan {
    pub previous_name: String,
    pub name: String,
}

/// The name a rename replaced and the name it stored.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RepoRenameResult {
    pub previous_name: String,
    pub name: String,
}

pub fn repo_constraints(repo_root: &Path) -> Result<RepoConstraints> {
    coordination::read(repo_root, || {
        Ok(load_repo_manifest(repo_root)?
            .1
            .constraints
            .unwrap_or_default())
    })
}

pub(crate) fn validate_existing_constraints(
    repo_root: &Path,
    constraints: &RepoConstraints,
    operation: &str,
) -> Result<()> {
    constraints.validate_definition()?;
    validate_repo_tag_constraints(repo_root, constraints)?;
    let (paths, _) = load_repo_manifest(repo_root)?;
    for id in super::inventory::list_repo_snapshot_ids(&paths)? {
        let manifest = super::snapshot_manifests::load_snapshot_manifest(&paths, &id)?;
        constraints.validate_snapshot_paths(&manifest, operation)?;
    }
    Ok(())
}

pub fn plan_set_repo_constraints(
    request: &RepoConstraintsSetRequest,
) -> Result<RepoConstraintsSetPlan> {
    request.control.checkpoint()?;
    coordination::read(&request.repo_root, || {
        let constraints = normalized_constraints(&request.repo_root, &request.constraints)?;
        Ok(RepoConstraintsSetPlan { constraints })
    })
}

/// Sorts and deduplicates targets, then checks the result against every stored snapshot.
fn normalized_constraints(
    repo_root: &Path,
    constraints: &RepoConstraints,
) -> Result<RepoConstraints> {
    let mut constraints = constraints.clone();
    constraints.targets.sort();
    constraints.targets.dedup();
    validate_existing_constraints(repo_root, &constraints, "change constraints")?;
    Ok(constraints)
}

pub fn plan_rename_repo(request: &RepoRenameRequest) -> Result<RepoRenamePlan> {
    request.control.checkpoint()?;
    coordination::read(&request.repo_root, || {
        let (_, manifest) = load_repo_manifest(&request.repo_root)?;
        Ok(RepoRenamePlan {
            previous_name: manifest.repository_name,
            name: validated_repo_name(&request.name)?,
        })
    })
}

fn validated_repo_name(name: &str) -> Result<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(crate::Error::repo("repo name cannot be empty"));
    }
    if trimmed.contains('/') || trimmed.contains('\\') {
        return Err(crate::Error::repo(
            "repo name cannot contain path separators",
        ));
    }
    Ok(trimmed.to_owned())
}

pub fn set_repo_constraints(
    request: &RepoConstraintsSetRequest,
) -> Result<RepoConstraintsSetResult> {
    request.control.checkpoint()?;
    let repo_root = request.repo_root.as_path();
    coordination::write(repo_root, || {
        let constraints = normalized_constraints(repo_root, &request.constraints)?;
        let (paths, mut manifest) = load_repo_manifest(repo_root)?;
        manifest.constraints = (!constraints.is_empty()).then(|| constraints.clone());
        write_repo_manifest(&paths, &manifest)?;
        Ok(RepoConstraintsSetResult { constraints })
    })
}

pub fn rename_repo(request: &RepoRenameRequest) -> Result<RepoRenameResult> {
    request.control.checkpoint()?;
    let repo_root = request.repo_root.as_path();
    coordination::write(repo_root, || {
        let name = validated_repo_name(&request.name)?;
        let (repo_paths, mut repo_manifest) = load_repo_manifest(repo_root)?;
        let previous_name = std::mem::replace(&mut repo_manifest.repository_name, name.clone());
        write_repo_manifest(&repo_paths, &repo_manifest)?;
        Ok(RepoRenameResult {
            previous_name,
            name,
        })
    })
}

/// Reads metadata only. Called before the outer transaction commits, so bulk
/// edits are checked as one final state and readers cannot see partial parity.
pub(crate) fn validate_repo_tag_constraints(
    repo_root: &Path,
    constraints: &RepoConstraints,
) -> Result<()> {
    validate_proposed_snapshot_tags(repo_root, constraints, &Default::default())
}

pub(crate) fn validate_proposed_snapshot_tags(
    repo_root: &Path,
    constraints: &RepoConstraints,
    replacements: &BTreeMap<String, SnapshotTagSet>,
) -> Result<()> {
    constraints.validate_definition()?;
    if constraints.snapshot_tags.key_value.is_empty()
        && !constraints.require_matching_tag_keys_across_snapshots
    {
        return Ok(());
    }
    let paths = super::paths::RepoPaths::from_repo_root(repo_root);
    if !paths.snapshots_root.exists() {
        return Ok(());
    }
    let mut expected_keys: Option<(String, BTreeSet<String>)> = None;
    let mut violations = Vec::new();
    for id in super::inventory::list_repo_snapshot_ids(&paths)? {
        let path = super::snapshot_manifests::snapshot_metadata_path(&paths, &id)?;
        let json = filesystem::read_to_string(&path)?;
        let snapshot =
            super::snapshot_manifests::load_snapshot_metadata_from_json_text(&id, &json)?;
        let tags = replacements.get(&id).unwrap_or(&snapshot.snapshot_tags);
        if let Err(error) = constraints.validate_snapshot_tags(tags, &id) {
            violations.push(error.to_string());
        }
        if constraints.require_matching_tag_keys_across_snapshots {
            let keys = tags.key_value.keys().cloned().collect::<BTreeSet<_>>();
            if let Some((first_id, expected)) = &expected_keys {
                if &keys != expected {
                    let missing: Vec<_> = expected.difference(&keys).cloned().collect();
                    let extra: Vec<_> = keys.difference(expected).cloned().collect();
                    violations.push(format!("snapshot '{id}' must match tag keys of '{first_id}': missing {missing:?}, extra {extra:?}; edit affected snapshots together"));
                }
            } else {
                expected_keys = Some((id.to_owned(), keys));
            }
        }
    }
    if violations.is_empty() {
        Ok(())
    } else {
        Err(crate::Error::repo(violations.join("\n")))
    }
}

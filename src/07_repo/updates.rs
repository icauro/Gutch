//! Repository rename and constraint updates, with tag validation.

use std::path::Path;

use crate::Result;
use crate::filesystem;
use crate::manifest::SnapshotTagSet;
use crate::manifest::constraints::RepositoryConstraints;
use crate::repo::coordination;
use crate::repo::paths::load_gutch_repo_manifest;
use crate::repo::paths::write_gutch_repo_manifest;
use std::collections::BTreeMap;
use std::collections::BTreeSet;

pub fn read_repository_constraints(repo_root: &Path) -> Result<RepositoryConstraints> {
    coordination::read(repo_root, || {
        Ok(load_gutch_repo_manifest(repo_root)?
            .1
            .constraints
            .unwrap_or_default())
    })
}

pub(crate) fn validate_existing_constraints(
    repo_root: &Path,
    constraints: &RepositoryConstraints,
    operation: &str,
) -> Result<()> {
    constraints.validate_definition()?;
    validate_repository_tag_constraints(repo_root, constraints)?;
    let (paths, _) = load_gutch_repo_manifest(repo_root)?;
    for id in super::inventory::list_repo_snapshot_ids(&paths)? {
        let manifest = super::snapshot_manifests::load_snapshot_manifest(&paths, &id)?;
        constraints.validate_snapshot_paths(&manifest, operation)?;
    }
    Ok(())
}

pub fn plan_repository_constraints(
    repo_root: &Path,
    mut constraints: RepositoryConstraints,
) -> Result<RepositoryConstraints> {
    coordination::read(repo_root, || {
        constraints.targets.sort();
        constraints.targets.dedup();
        validate_existing_constraints(repo_root, &constraints, "change constraints")?;
        Ok(constraints)
    })
}

pub fn plan_repo_rename(repo_root: &Path, name: &str) -> Result<String> {
    coordination::read(repo_root, || {
        load_gutch_repo_manifest(repo_root)?;
        validated_repo_name(name)
    })
}

fn validated_repo_name(name: &str) -> Result<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(crate::Error::repository("repo name cannot be empty"));
    }
    if trimmed.contains('/') || trimmed.contains('\\') {
        return Err(crate::Error::repository(
            "repo name cannot contain path separators",
        ));
    }
    Ok(trimmed.to_owned())
}

pub fn update_repository_constraints(
    repo_root: &Path,
    mut constraints: RepositoryConstraints,
) -> Result<()> {
    coordination::write(repo_root, || {
        constraints.targets.sort();
        constraints.targets.dedup();
        validate_existing_constraints(repo_root, &constraints, "change constraints")?;
        let (paths, mut manifest) = load_gutch_repo_manifest(repo_root)?;
        manifest.constraints = if constraints.is_empty() {
            None
        } else {
            Some(constraints)
        };
        write_gutch_repo_manifest(&paths, &manifest)
    })
}

pub fn rename_repo(repo_root: &Path, name: &str) -> Result<()> {
    coordination::write(repo_root, || {
        let name = validated_repo_name(name)?;
        let (repo_paths, mut repo_manifest) = load_gutch_repo_manifest(repo_root)?;
        repo_manifest.name = name;
        write_gutch_repo_manifest(&repo_paths, &repo_manifest)
    })
}

/// Reads metadata only. Called before the outer transaction commits, so bulk
/// edits are checked as one final state and readers cannot see partial parity.
pub(crate) fn validate_repository_tag_constraints(
    repo_root: &Path,
    constraints: &RepositoryConstraints,
) -> Result<()> {
    validate_proposed_snapshot_tags(repo_root, constraints, &Default::default())
}

pub(crate) fn validate_proposed_snapshot_tags(
    repo_root: &Path,
    constraints: &RepositoryConstraints,
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
        Err(crate::Error::repository(violations.join("\n")))
    }
}

use std::path::Path;

use crate::Result;
use crate::manifest::constraints::RepositoryConstraints;
use crate::repo::paths::load_gutch_repo_manifest;
use crate::repo::paths::write_gutch_repo_manifest;

pub fn read_repository_constraints(repo_root: &Path) -> Result<RepositoryConstraints> {
    crate::repo::coordination::read(repo_root, || {
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
    let (paths, _) = load_gutch_repo_manifest(repo_root)?;
    for entry in walkdir::WalkDir::new(&paths.snapshots_root).sort_by_file_name() {
        let entry = entry?;
        if !entry.file_type().is_file()
            || entry.file_name() != super::snapshot_manifests::SNAPSHOT_METADATA_FILE
        {
            continue;
        }
        let id = entry
            .path()
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|p| p.to_str())
            .ok_or_else(|| crate::Error::manifest("invalid snapshot directory"))?;
        let manifest = super::snapshot_manifests::load_snapshot_manifest(&paths, id)?;
        crate::filesystem::constraints::validate_paths(
            constraints,
            manifest.entries.iter().map(|entry| entry.path()),
            operation,
            Some(id),
        )?;
    }
    Ok(())
}

pub fn plan_repository_constraints(
    repo_root: &Path,
    mut constraints: RepositoryConstraints,
) -> Result<RepositoryConstraints> {
    crate::repo::coordination::read(repo_root, || {
        constraints.targets.sort();
        constraints.targets.dedup();
        validate_existing_constraints(repo_root, &constraints, "change constraints")?;
        Ok(constraints)
    })
}

pub fn plan_repo_rename(repo_root: &Path, name: &str) -> Result<String> {
    crate::repo::coordination::read(repo_root, || {
        load_gutch_repo_manifest(repo_root)?;
        validated_repo_name(name)
    })
}

fn validated_repo_name(name: &str) -> Result<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(crate::Error::manifest("repo name cannot be empty"));
    }
    if trimmed.contains('/') || trimmed.contains('\\') {
        return Err(crate::Error::manifest(
            "repo name cannot contain path separators",
        ));
    }
    Ok(trimmed.to_owned())
}

pub fn update_repository_constraints(
    repo_root: &Path,
    mut constraints: RepositoryConstraints,
) -> Result<()> {
    crate::repo::coordination::write(repo_root, || {
        constraints.targets.sort();
        constraints.targets.dedup();
        validate_existing_constraints(repo_root, &constraints, "change constraints")?;
        let (paths, mut manifest) = load_gutch_repo_manifest(repo_root)?;
        manifest.constraints = if constraints.targets.is_empty() {
            None
        } else {
            Some(constraints)
        };
        write_gutch_repo_manifest(&paths, &manifest)
    })
}

pub fn rename_repo(repo_root: &Path, name: &str) -> Result<()> {
    let operation_root = (repo_root).to_path_buf();
    crate::repo::coordination::write(&operation_root, || {
        let name = validated_repo_name(name)?;
        let (repo_paths, mut repo_manifest) = load_gutch_repo_manifest(repo_root)?;
        repo_manifest.name = name;
        write_gutch_repo_manifest(&repo_paths, &repo_manifest)
    })
}

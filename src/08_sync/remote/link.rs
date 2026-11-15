//! Binding a local repository to a remote one and listing or creating remotes.

use super::provider::RemoteRepositoryCommandProvider;
use super::provider::RemoteRepositorySummary;
use super::provider::RemoteTagReferencesResult;
use crate::Error;
use crate::Result;
use crate::repo::coordination;
use crate::repo::paths::RepoPaths;
use crate::repo::paths::load_gutch_repo_manifest;
use crate::repo::paths::write_gutch_repo_manifest;
use crate::repo::tag_references::advisory_tag_references;
use std::path::Path;
pub(crate) fn linked_remote_repo_id(repo_root: &Path) -> Result<String> {
    let (_paths, manifest) = load_gutch_repo_manifest(repo_root)?;
    if !manifest.is_linked() {
        return Err(Error::repository(format!(
            "repo '{}' is not linked. Use 'gutch repo remote create' first.",
            repo_root.display()
        )));
    }
    if manifest.remote_repo_id.trim().is_empty() {
        return Err(Error::repository("linked repo is missing remote_repo_id"));
    }
    Ok(manifest.remote_repo_id)
}

pub fn bind_remote_repo(repo_root: &Path, remote_repo_id: &str) -> Result<()> {
    let operation_root = (repo_root).to_path_buf();
    coordination::write(&operation_root, || {
        let remote_repo_id = remote_repo_id.trim();
        if remote_repo_id.is_empty() {
            return Err(Error::repository("remote repo ID is required"));
        }

        let (paths, mut manifest) = load_gutch_repo_manifest(repo_root)?;
        manifest.remote_repo_id = remote_repo_id.to_string();
        write_gutch_repo_manifest(&paths, &manifest)
    })
}

pub fn list_remote_repositories(
    provider: &dyn RemoteRepositoryCommandProvider,
) -> Result<Vec<RemoteRepositorySummary>> {
    provider.list_remote_repositories()
}

pub fn create_remote_repo(
    repo_root: &Path,
    scope_id: &str,
    provider: &dyn RemoteRepositoryCommandProvider,
) -> Result<()> {
    let operation_root = (repo_root).to_path_buf();
    coordination::write(&operation_root, || {
        let (paths, mut manifest) = load_gutch_repo_manifest(repo_root)?;
        if manifest.is_linked() {
            return Err(Error::repository(format!(
                "repo '{}' is already linked (remote_repo_id={}). Unlink first.",
                paths.repo_root().display(),
                manifest.remote_repo_id
            )));
        }

        let scope_id = scope_id.trim();
        if scope_id.is_empty() {
            return Err(Error::repository("remote repo scope is required"));
        }
        let remote_repo_id = provider.create_remote_repository(scope_id, &manifest.name)?;
        if remote_repo_id.trim().is_empty() {
            return Err(Error::repository(
                "remote provider returned an empty repo ID",
            ));
        }
        manifest.remote_repo_id = remote_repo_id;
        write_gutch_repo_manifest(&paths, &manifest)?;
        Ok(())
    })
}

pub fn unbind_remote_repo(
    repo_root: &Path,
    delete_remote: bool,
    provider: Option<&dyn RemoteRepositoryCommandProvider>,
) -> Result<()> {
    let operation_root = (repo_root).to_path_buf();
    coordination::write(&operation_root, || {
        let (paths, mut manifest) = load_gutch_repo_manifest(repo_root)?;
        if !manifest.is_linked() {
            return Ok(());
        }

        if delete_remote {
            let remote_repo_id = manifest.remote_repo_id.trim();
            if remote_repo_id.is_empty() {
                return Err(Error::repository(format!(
                    "invalid remote_repo_id '{}' in manifest",
                    manifest.remote_repo_id
                )));
            }
            let remote_repo_id = remote_repo_id.to_string();
            let provider = provider.ok_or_else(|| {
                Error::repository(
                    "remote provider context is required when deleting the remote repo",
                )
            })?;
            let repository = provider
                .repository_provider()
                .resolve_repository(None, Some(&remote_repo_id))?;
            provider.delete_remote_repository(&repository)?;
        }

        manifest.remote_repo_id.clear();
        write_gutch_repo_manifest(&paths, &manifest)?;
        Ok(())
    })
}

pub fn inspect_repo_tag_references(
    repo_root: &Path,
    provider: &dyn RemoteRepositoryCommandProvider,
) -> Result<RemoteTagReferencesResult> {
    let (repo_paths, repo_manifest) = load_gutch_repo_manifest(repo_root)?;
    let remote_repo_id = repo_manifest.remote_repo_id.trim();
    if remote_repo_id.is_empty() {
        return Err(Error::repository(format!(
            "repo '{}' is not linked. Use 'gutch repo remote create' first.",
            repo_paths.repo_root().display()
        )));
    }
    let remote_repo_id = remote_repo_id.to_string();
    let repository = provider
        .repository_provider()
        .resolve_repository(None, Some(&remote_repo_id))?;
    let references = provider.referenced_tag_references(&repository)?;
    let referenced_tags = advisory_tag_references(&references);

    Ok(RemoteTagReferencesResult {
        repo_root: repo_paths.repo_root().display().to_string(),
        remote_repo_id,
        referenced_tags,
    })
}

pub(super) fn bound_remote_repo_id(repo_root: &Path) -> Result<Option<String>> {
    let repo_paths = RepoPaths::from_repo_root(repo_root);
    if !repo_paths.repo_metadata_path.exists() {
        return Ok(None);
    }
    let (_paths, manifest) = load_gutch_repo_manifest(repo_root)?;
    Ok(manifest.is_linked().then_some(manifest.remote_repo_id))
}

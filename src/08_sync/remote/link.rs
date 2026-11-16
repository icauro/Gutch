//! Binding a local repository to a remote one and listing or creating remotes.

use super::provider::RemoteRepoService;
use super::provider::RemoteTagReferencesResult;
use crate::Error;
use crate::Result;
use crate::progress::OperationControl;
use crate::repo::coordination;
use crate::repo::paths::RepoPaths;
use crate::repo::paths::load_repo_manifest;
use crate::repo::paths::write_repo_manifest;
use crate::repo::tag_references::advisory_tag_references;
use serde::Serialize;
use std::path::Path;
use std::path::PathBuf;

/// Records an existing remote repository ID as the remote of a local repository.
#[derive(Debug, Clone, Default)]
pub struct RemoteRepoBindRequest {
    pub repo_root: PathBuf,
    pub remote_repo_id: String,
    pub control: OperationControl,
}

#[derive(Debug, Clone, Serialize)]
pub struct RemoteRepoBindResult {
    pub remote_repo_id: String,
}

/// Creates a remote repository in a provider scope and binds the local repository to it.
/// The provider service is passed beside the request.
#[derive(Debug, Clone, Default)]
pub struct RemoteRepoCreateRequest {
    pub repo_root: PathBuf,
    pub scope_id: String,
    pub control: OperationControl,
}

#[derive(Debug, Clone, Serialize)]
pub struct RemoteRepoCreateResult {
    pub remote_repo_id: String,
}

/// Removes the remote binding, and optionally deletes the remote repository.
#[derive(Debug, Clone, Default)]
pub struct RemoteRepoUnbindRequest {
    pub repo_root: PathBuf,
    pub delete_remote: bool,
    pub control: OperationControl,
}

#[derive(Debug, Clone, Serialize)]
pub struct RemoteRepoUnbindResult {
    /// The remote that was bound, or `None` when nothing was bound.
    pub remote_repo_id: Option<String>,
    pub remote_deleted: bool,
}

pub(crate) fn linked_remote_repo_id(repo_root: &Path) -> Result<String> {
    let (_paths, manifest) = load_repo_manifest(repo_root)?;
    manifest.remote_repo_id.ok_or_else(|| not_linked(repo_root))
}

fn not_linked(repo_root: &Path) -> Error {
    Error::repo(format!(
        "repo '{}' is not linked. Use 'gutch repo remote create' first.",
        repo_root.display()
    ))
}

pub fn bind_remote_repo(request: &RemoteRepoBindRequest) -> Result<RemoteRepoBindResult> {
    let repo_root = request.repo_root.as_path();
    coordination::write_with_control(repo_root, &request.control, || {
        request.control.checkpoint()?;
        let remote_repo_id = request.remote_repo_id.trim();
        if remote_repo_id.is_empty() {
            return Err(Error::repo("remote repo ID is required"));
        }

        let (paths, mut manifest) = load_repo_manifest(repo_root)?;
        manifest.remote_repo_id = Some(remote_repo_id.to_string());
        write_repo_manifest(&paths, &manifest)?;
        Ok(RemoteRepoBindResult {
            remote_repo_id: remote_repo_id.to_string(),
        })
    })
}

pub fn create_remote_repo(
    request: &RemoteRepoCreateRequest,
    provider: &dyn RemoteRepoService,
) -> Result<RemoteRepoCreateResult> {
    let repo_root = request.repo_root.as_path();
    coordination::write_with_control(repo_root, &request.control, || {
        request.control.checkpoint()?;
        let (paths, mut manifest) = load_repo_manifest(repo_root)?;
        if let Some(remote_repo_id) = &manifest.remote_repo_id {
            return Err(Error::repo(format!(
                "repo '{}' is already linked (remote_repo_id={remote_repo_id}). Unlink first.",
                paths.repo_root().display(),
            )));
        }

        let scope_id = request.scope_id.trim();
        if scope_id.is_empty() {
            return Err(Error::repo("remote repo scope is required"));
        }
        let remote_repo_id = provider.create_repo(scope_id, &manifest.repository_name)?;
        if remote_repo_id.trim().is_empty() {
            return Err(Error::repo("remote provider returned an empty repo ID"));
        }
        manifest.remote_repo_id = Some(remote_repo_id.clone());
        write_repo_manifest(&paths, &manifest)?;
        Ok(RemoteRepoCreateResult { remote_repo_id })
    })
}

pub fn unbind_remote_repo(
    request: &RemoteRepoUnbindRequest,
    provider: Option<&dyn RemoteRepoService>,
) -> Result<RemoteRepoUnbindResult> {
    let repo_root = request.repo_root.as_path();
    coordination::write_with_control(repo_root, &request.control, || {
        request.control.checkpoint()?;
        let (paths, mut manifest) = load_repo_manifest(repo_root)?;
        let Some(remote_repo_id) = manifest.remote_repo_id.take() else {
            return Ok(RemoteRepoUnbindResult {
                remote_repo_id: None,
                remote_deleted: false,
            });
        };

        if request.delete_remote {
            let provider = provider
                .ok_or_else(|| Error::repo("deleting the remote repo needs a remote provider"))?;
            let repository = provider
                .repo_provider()
                .resolve_repo(None, Some(&remote_repo_id))?;
            provider.delete_repo(&repository)?;
        }

        write_repo_manifest(&paths, &manifest)?;
        Ok(RemoteRepoUnbindResult {
            remote_repo_id: Some(remote_repo_id),
            remote_deleted: request.delete_remote,
        })
    })
}

pub fn remote_tag_references(
    repo_root: &Path,
    provider: &dyn RemoteRepoService,
) -> Result<RemoteTagReferencesResult> {
    let (repo_paths, repo_manifest) = load_repo_manifest(repo_root)?;
    let remote_repo_id = repo_manifest
        .remote_repo_id
        .ok_or_else(|| not_linked(repo_paths.repo_root()))?;
    let repository = provider
        .repo_provider()
        .resolve_repo(None, Some(&remote_repo_id))?;
    let references = provider.tag_references(&repository)?;
    let referenced_tags = advisory_tag_references(&references);

    Ok(RemoteTagReferencesResult {
        repo_root: repo_paths.repo_root().to_path_buf(),
        remote_repo_id,
        referenced_tags,
    })
}

pub(super) fn bound_remote_repo_id(repo_root: &Path) -> Result<Option<String>> {
    let repo_paths = RepoPaths::from_repo_root(repo_root);
    if !repo_paths.repo_metadata_path.exists() {
        return Ok(None);
    }
    let (_paths, manifest) = load_repo_manifest(repo_root)?;
    Ok(manifest.remote_repo_id)
}

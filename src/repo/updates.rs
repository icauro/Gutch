use std::path::Path;

use crate::Result;
use crate::manifest;

use super::{load_gutch_repo_manifest, write_gutch_repo_manifest};

pub fn initialize_tracked_gutch_repo(
    repo_root: &Path,
    snapshot_tag_schema: Option<manifest::SnapshotTagSchema>,
    snapshot_sources_path: Option<String>,
) -> Result<()> {
    super::init_policy::initialize_tracked_gutch_repo_with_options(
        repo_root,
        snapshot_tag_schema,
        snapshot_sources_path,
        super::init_policy::RepoInitOptions { force: false },
    )
}

pub fn update_gutch_repo_schema(
    repo_root: &Path,
    snapshot_tag_schema: Option<manifest::SnapshotTagSchema>,
) -> Result<()> {
    let (repo_paths, mut repo_manifest) = load_gutch_repo_manifest(repo_root)?;
    repo_manifest.snapshot_tag_schema = Some(snapshot_tag_schema.unwrap_or_default());
    write_gutch_repo_manifest(&repo_paths, &repo_manifest)
}

pub fn update_gutch_repo_name(repo_root: &Path, name: &str) -> Result<()> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(crate::Error::manifest("repo name cannot be empty"));
    }
    if trimmed.contains('/') || trimmed.contains('\\') {
        return Err(crate::Error::manifest(
            "repo name cannot contain path separators",
        ));
    }

    let (repo_paths, mut repo_manifest) = load_gutch_repo_manifest(repo_root)?;
    repo_manifest.name = trimmed.to_string();
    write_gutch_repo_manifest(&repo_paths, &repo_manifest)
}

pub fn bind_gutch_repo_remote(
    repo_root: &Path,
    product_id: i64,
    remote_repo_id: i64,
) -> Result<()> {
    if product_id <= 0 {
        return Err(crate::Error::manifest(
            "product_id must be greater than zero",
        ));
    }
    if remote_repo_id <= 0 {
        return Err(crate::Error::manifest(
            "remote_repo_id must be greater than zero",
        ));
    }

    let (repo_paths, mut repo_manifest) = load_gutch_repo_manifest(repo_root)?;
    repo_manifest.product_id = product_id.to_string();
    repo_manifest.remote_repo_id = remote_repo_id.to_string();
    repo_manifest.sync_product_metadata_from_binding();
    write_gutch_repo_manifest(&repo_paths, &repo_manifest)
}

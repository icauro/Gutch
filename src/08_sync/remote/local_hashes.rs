//! Hashes of the local repo files that a push or pull compares with the remote.
//! A missing local file hashes to an empty string.

use super::artifact_decision::remote_artifact_hash;
use crate::Result;
use crate::artifact::hash::hash_hex;
use crate::artifact::integrity::hash_file;
use crate::filesystem;
use crate::manifest::SnapshotManifest;
use crate::repo::paths::RepoPaths;
use crate::repo::snapshot_manifests::load_snapshot_manifest;
use crate::repo::snapshot_manifests::serialize_snapshot_metadata_for_remote;
use crate::repo::snapshot_manifests::serialize_snapshot_product_metadata_for_remote;
use crate::repo::snapshot_manifests::snapshot_blueprint_path;
use crate::repo::snapshot_manifests::snapshot_metadata_path;
use std::path::Path;

pub(super) fn local_repo_metadata_hash_b3(repo_paths: &RepoPaths) -> Result<String> {
    existing_file_hash(&repo_paths.repo_metadata_path)
}

pub(super) fn local_chunk_map_hash_b3(repo_paths: &RepoPaths) -> Result<String> {
    existing_file_hash(&repo_paths.chunk_map_path)
}

pub(super) fn local_snapshot_metadata_hash_b3(
    repo_paths: &RepoPaths,
    snapshot_id: &str,
) -> Result<String> {
    let Some(manifest) = load_local_snapshot(repo_paths, snapshot_id)? else {
        return Ok(String::new());
    };
    Ok(remote_artifact_hash(
        &serialize_snapshot_metadata_for_remote(&manifest)?,
    ))
}

pub(super) fn local_snapshot_product_metadata_hash_b3(
    repo_paths: &RepoPaths,
    snapshot_id: &str,
) -> Result<String> {
    let Some(manifest) = load_local_snapshot(repo_paths, snapshot_id)? else {
        return Ok(String::new());
    };
    let Some(bytes) = serialize_snapshot_product_metadata_for_remote(&manifest)? else {
        return Ok(String::new());
    };
    Ok(remote_artifact_hash(&bytes))
}

pub(super) fn local_snapshot_blueprint_hash_b3(
    repo_paths: &RepoPaths,
    snapshot_id: &str,
) -> Result<String> {
    let path = snapshot_blueprint_path(repo_paths, snapshot_id)?;
    if !path.exists() {
        return Ok(String::new());
    }
    Ok(hash_hex(&hash_file(&path)?))
}

fn existing_file_hash(path: &Path) -> Result<String> {
    if !path.exists() {
        return Ok(String::new());
    }
    Ok(remote_artifact_hash(&filesystem::read(path)?))
}

/// Loads the snapshot only when both its metadata and blueprint exist locally.
fn load_local_snapshot(
    repo_paths: &RepoPaths,
    snapshot_id: &str,
) -> Result<Option<SnapshotManifest>> {
    if !snapshot_metadata_path(repo_paths, snapshot_id)?.exists()
        || !snapshot_blueprint_path(repo_paths, snapshot_id)?.exists()
    {
        return Ok(None);
    }
    load_snapshot_manifest(repo_paths, snapshot_id).map(Some)
}

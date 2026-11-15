//! Hashes of the local repo files that a push or pull compares with the remote.

use super::artifact_decision::repo_file_hash;
use crate::Result;
use crate::artifact::hash::hash_hex;
use crate::artifact::integrity::hash_file;
use crate::filesystem;
use crate::repo::paths::RepoPaths;
use crate::repo::snapshot_manifests::load_snapshot_manifest;
use crate::repo::snapshot_manifests::serialize_snapshot_metadata_for_remote;
use crate::repo::snapshot_manifests::serialize_snapshot_product_metadata_for_remote;
use crate::repo::snapshot_manifests::snapshot_blueprint_path;
use crate::repo::snapshot_manifests::snapshot_metadata_path;

pub(super) fn local_repo_metadata_hash_b3(repo_paths: &RepoPaths) -> Result<String> {
    if !repo_paths.repo_metadata_path.exists() {
        return Ok(String::new());
    }
    Ok(repo_file_hash(&filesystem::read(
        &repo_paths.repo_metadata_path,
    )?))
}

pub(super) fn local_chunk_map_hash_b3(repo_paths: &RepoPaths) -> Result<String> {
    if !repo_paths.chunk_map_path.exists() {
        return Ok(String::new());
    }
    Ok(repo_file_hash(&filesystem::read(
        &repo_paths.chunk_map_path,
    )?))
}

pub(super) fn local_snapshot_metadata_hash_b3(
    repo_paths: &RepoPaths,
    snapshot_id: &str,
) -> Result<String> {
    let metadata_path = snapshot_metadata_path(repo_paths, snapshot_id)?;
    let blueprint_path = snapshot_blueprint_path(repo_paths, snapshot_id)?;
    if !metadata_path.exists() || !blueprint_path.exists() {
        return Ok(String::new());
    }
    let manifest = load_snapshot_manifest(repo_paths, snapshot_id)?;
    Ok(repo_file_hash(&serialize_snapshot_metadata_for_remote(
        &manifest,
    )?))
}

pub(super) fn local_snapshot_product_metadata_hash_b3(
    repo_paths: &RepoPaths,
    snapshot_id: &str,
) -> Result<String> {
    let metadata_path = snapshot_metadata_path(repo_paths, snapshot_id)?;
    let blueprint_path = snapshot_blueprint_path(repo_paths, snapshot_id)?;
    if !metadata_path.exists() || !blueprint_path.exists() {
        return Ok(String::new());
    }
    let manifest = load_snapshot_manifest(repo_paths, snapshot_id)?;
    let Some(bytes) = serialize_snapshot_product_metadata_for_remote(&manifest)? else {
        return Ok(String::new());
    };
    Ok(repo_file_hash(&bytes))
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

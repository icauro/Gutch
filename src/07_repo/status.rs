//! Repository status and health checks of stored snapshots.

use std::collections::HashSet;
use std::path::Path;

use serde::Deserialize;
use serde::Serialize;

use super::inventory::list_repo_snapshot_ids;
use super::inventory::scan_artifact_inventory;
use super::snapshot_tag_summary::summarize_snapshot_tags;
use crate::Error;
use crate::Result;
use crate::artifact::hash::hash_hex;
use crate::artifact::reference::ArtifactRef;
use crate::manifest;
use crate::manifest::REPO_METADATA_FILE;
use crate::repo::artifacts::RequiredArtifacts;
use crate::repo::artifacts::classify_required_artifacts;
use crate::repo::coordination;
use crate::repo::index::load_chunk_index;
use crate::repo::paths::RepoPaths;
use crate::repo::paths::load_gutch_repo_manifest;
use crate::repo::snapshot_manifests::load_snapshot_manifest;
use crate::repo::snapshot_manifests::serialize_snapshot_product_metadata_for_remote;
use crate::repo::snapshot_manifests::snapshot_metadata_path;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoStatus {
    pub exists: bool,
    pub is_repo: bool,
    pub repo_path: String,
    pub name: String,
    pub remote_repo_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_manifest: Option<serde_json::Value>,
    pub snapshots: Vec<RepoSnapshotStatus>,
    #[serde(default)]
    pub bundle_count: usize,
    #[serde(default)]
    pub bundle_bytes: u64,
    #[serde(default)]
    pub artifact_count: usize,
    #[serde(default)]
    pub artifact_bytes: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoSnapshotStatus {
    pub snapshot_id: String,
    pub version_tag: String,
    pub os: String,
    pub arch: String,
    pub file_count: usize,
    pub required_bundle_count: usize,
    #[serde(default)]
    pub required_artifact_count: usize,
    pub total_logical_size: u64,
    pub manifest_path: String,
    pub missing_bundle_count: usize,
    #[serde(default)]
    pub missing_artifact_count: usize,
    pub source_modified_at: String,
    #[serde(rename = "tags")]
    pub snapshot_tags: manifest::SnapshotTagSet,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_metadata: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_product_metadata_hash_b3: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_product_metadata: Option<manifest::SnapshotProductMetadata>,
    #[serde(default)]
    pub file_paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch_readiness_error: Option<String>,
}

pub fn read_repo_status(repo_root: &Path) -> Result<RepoStatus> {
    if !repo_root.is_dir() {
        return check_gutch_repo_unlocked(repo_root);
    }
    coordination::read(repo_root, || check_gutch_repo_unlocked(repo_root))
}

fn check_gutch_repo_unlocked(repo_root: &Path) -> Result<RepoStatus> {
    let repo_path = repo_root.to_path_buf();
    let repo_path_display = repo_path.display().to_string();
    if !repo_path.exists() {
        return Ok(empty_repo_check_status(
            repo_path_display,
            false,
            Some("Path does not exist.".to_string()),
        ));
    }
    if !repo_path.is_dir() {
        return Ok(empty_repo_check_status(
            repo_path_display,
            true,
            Some("Path is not a directory.".to_string()),
        ));
    }

    let repo_metadata_path = repo_path.join(REPO_METADATA_FILE);
    if !repo_metadata_path.exists() {
        return Ok(empty_repo_check_status(
            repo_path_display,
            true,
            Some("repo_metadata.json was not found.".to_string()),
        ));
    }

    let (repo_paths, repo_manifest) = load_gutch_repo_manifest(&repo_path)?;

    let inventory = scan_artifact_inventory(&repo_paths)?;

    let snapshots = check_snapshot_manifests(
        &repo_paths,
        &repo_paths.snapshots_root,
        &inventory.artifacts,
    )?;

    Ok(RepoStatus {
        exists: true,
        is_repo: true,
        repo_path: repo_path_display,
        name: repo_manifest.name.clone(),
        remote_repo_id: repo_manifest.remote_repo_id.clone(),
        repo_manifest: Some(serde_json::to_value(&repo_manifest)?),
        snapshots,
        bundle_count: inventory.bundle_count,
        bundle_bytes: inventory.bundle_bytes,
        artifact_count: inventory.artifact_count,
        artifact_bytes: inventory.artifact_bytes,
        error: None,
    })
}

fn empty_repo_check_status(repo_path: String, exists: bool, error: Option<String>) -> RepoStatus {
    RepoStatus {
        exists,
        is_repo: false,
        repo_path,
        name: String::new(),
        remote_repo_id: None,
        repo_manifest: None,
        snapshots: Vec::new(),
        bundle_count: 0,
        bundle_bytes: 0,
        artifact_count: 0,
        artifact_bytes: 0,
        error,
    }
}

fn check_snapshot_manifests(
    repo_paths: &RepoPaths,
    snapshots_root: &Path,
    available_artifacts: &HashSet<ArtifactRef>,
) -> Result<Vec<RepoSnapshotStatus>> {
    if !snapshots_root.exists() {
        return Ok(Vec::new());
    }

    let chunk_index = load_chunk_index(repo_paths)?;

    let mut snapshots = Vec::new();
    for snapshot_id in list_repo_snapshot_ids(repo_paths)? {
        let manifest_path = snapshot_metadata_path(repo_paths, &snapshot_id)?;
        let manifest = load_snapshot_manifest(repo_paths, &snapshot_id)?;
        let snapshot_metadata =
            Some(serde_json::to_value(manifest.metadata()).map_err(|error| {
                Error::repository(format!("failed to serialize snapshot metadata: {error}"))
            })?);
        let snapshot_product_metadata_hash_b3 =
            serialize_snapshot_product_metadata_for_remote(&manifest)?
                .map(|bytes| hash_hex(&blake3::hash(&bytes)));
        let RequiredArtifacts {
            artifacts: required_artifacts,
            unindexed_chunks,
        } = classify_required_artifacts(&manifest, &chunk_index)?;
        let required_bundle_count = required_artifacts
            .iter()
            .filter(|artifact| artifact.is_bundle())
            .count();
        let missing_bundle_count = required_artifacts
            .iter()
            .filter(|artifact| artifact.is_bundle())
            .filter(|artifact| !available_artifacts.contains(*artifact))
            .count();
        // An unindexed chunk has no known artifact, so it is missing content.
        let missing_artifact_count = required_artifacts
            .iter()
            .filter(|artifact| !available_artifacts.contains(*artifact))
            .count()
            + unindexed_chunks.len();
        let launch_readiness_error =
            if manifest.product_metadata.is_some() && manifest.launch.is_none() {
                Some("No runtime target matches this host.".to_string())
            } else {
                None
            };

        let summary = summarize_snapshot_tags(&manifest.snapshot_tags);
        snapshots.push(RepoSnapshotStatus {
            snapshot_id: manifest.snapshot_id.clone(),
            version_tag: summary.version_tag,
            os: summary.os,
            arch: summary.arch,
            file_count: manifest.file_count(),
            required_bundle_count,
            required_artifact_count: required_artifacts.len() + unindexed_chunks.len(),
            total_logical_size: manifest.total_logical_size,
            manifest_path: manifest_path.display().to_string(),
            missing_bundle_count,
            missing_artifact_count,
            source_modified_at: manifest.source_modified_at.clone(),
            snapshot_tags: manifest.snapshot_tags.clone(),
            snapshot_metadata,
            snapshot_product_metadata_hash_b3,
            snapshot_product_metadata: manifest.product_metadata.clone(),
            file_paths: manifest.files().map(|file| file.path.clone()).collect(),
            launch_readiness_error,
        });
    }

    snapshots.sort_by(|left, right| left.snapshot_id.cmp(&right.snapshot_id));

    Ok(snapshots)
}

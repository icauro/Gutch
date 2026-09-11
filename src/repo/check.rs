use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::inventory::{list_repo_snapshot_ids, scan_artifact_inventory, scan_bundle_inventory};
use super::snapshot_descriptor::summarize_snapshot_tags;
use super::{
    GutchRepoPaths, load_chunk_index, load_gutch_repo_manifest, load_snapshot_manifest,
    required_bundle_ids, source_modified_at_from_directory,
};
use crate::manifest::REPO_METADATA_FILE;
use crate::manifest::{self};
use crate::{Error, Result};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoCheckStatus {
    pub exists: bool,
    pub is_repo: bool,
    pub repo_path: String,
    pub name: String,
    pub remote_repo_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_manifest: Option<serde_json::Value>,
    pub snapshots: Vec<RepoCheckSnapshotStatus>,
    #[serde(default)]
    pub bundle_count: usize,
    #[serde(default)]
    pub bundle_bytes: u64,
    #[serde(default)]
    pub artifact_count: usize,
    #[serde(default)]
    pub artifact_bytes: u64,
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) timings: Option<RepoCheckTimings>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RepoCheckTimings {
    pub(crate) load_manifest_ms: u64,
    pub(crate) scan_bundles_ms: u64,
    pub(crate) scan_artifacts_ms: u64,
    pub(crate) load_chunk_index_ms: u64,
    pub(crate) load_snapshot_manifests_ms: u64,
    pub(crate) snapshot_count: usize,
    pub(crate) total_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoCheckSnapshotStatus {
    pub snapshot_id: String,
    pub version_id: String,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceSnapshotOperationCandidate {
    pub path: String,
    pub folder_name: String,
    #[serde(default)]
    pub source_modified_at: String,
    pub version_id: String,
    pub os: String,
    pub arch: String,
    pub snapshot_tags: manifest::SnapshotTagSet,
    #[serde(default)]
    pub disposition: SourceSnapshotOperationDisposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceSnapshotOperationDisposition {
    #[default]
    Unchanged,
    Capture,
    Invalid,
}

pub fn check_gutch_repo(repo_root: &Path) -> Result<RepoCheckStatus> {
    if !repo_root.is_dir() {
        return check_gutch_repo_unlocked(repo_root);
    }
    crate::repo::coordination::read(repo_root, || check_gutch_repo_unlocked(repo_root))
}

fn check_gutch_repo_unlocked(repo_root: &Path) -> Result<RepoCheckStatus> {
    let total_start = std::time::Instant::now();
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

    let manifest_start = std::time::Instant::now();
    let (repo_paths, repo_manifest) = load_gutch_repo_manifest(&repo_path)?;
    let load_manifest_ms = manifest_start.elapsed().as_millis() as u64;

    let bundle_scan_start = std::time::Instant::now();
    let (_available_bundle_ids, bundle_count, bundle_bytes) =
        scan_bundle_inventory(&repo_paths.bundles_root)?;
    let scan_bundles_ms = bundle_scan_start.elapsed().as_millis() as u64;
    let artifact_scan_start = std::time::Instant::now();
    let (available_artifacts, artifact_count, artifact_bytes) =
        scan_artifact_inventory(&repo_paths)?;
    let scan_artifacts_ms = artifact_scan_start.elapsed().as_millis() as u64;

    let (snapshots, snapshot_check_timings) = check_snapshot_manifests(
        &repo_paths,
        &repo_paths.snapshots_root,
        &available_artifacts,
    )?;
    let snapshot_count = snapshots.len();

    let total_ms = total_start.elapsed().as_millis() as u64;

    Ok(RepoCheckStatus {
        exists: true,
        is_repo: true,
        repo_path: repo_path_display,
        name: repo_manifest.name.clone(),
        remote_repo_id: repo_manifest.remote_repo_id.clone(),
        repo_manifest: Some(serde_json::to_value(&repo_manifest)?),
        snapshots,
        bundle_count,
        bundle_bytes,
        artifact_count,
        artifact_bytes,
        error: None,
        timings: Some(RepoCheckTimings {
            load_manifest_ms,
            scan_bundles_ms,
            scan_artifacts_ms,
            load_chunk_index_ms: snapshot_check_timings.load_chunk_index_ms,
            load_snapshot_manifests_ms: snapshot_check_timings.load_snapshot_manifests_ms,
            snapshot_count,
            total_ms,
        }),
    })
}

pub fn discover_source_snapshot_operations(
    repo_root: &Path,
    source_snapshots_root: &Path,
) -> Result<Vec<SourceSnapshotOperationCandidate>> {
    if !source_snapshots_root.exists() {
        return Err(Error::manifest(format!(
            "source snapshots path does not exist: {}",
            source_snapshots_root.display()
        )));
    }
    if !source_snapshots_root.is_dir() {
        return Err(Error::manifest(format!(
            "source snapshots path is not a directory: {}",
            source_snapshots_root.display()
        )));
    }

    load_gutch_repo_manifest(repo_root)?;
    let config =
        crate::snapshot::source_config::SourceConfig::load_or_default(source_snapshots_root)?;
    let mut candidates = Vec::new();

    for snapshot in config.discover_snapshot_sources(source_snapshots_root)? {
        let explicit_tags = snapshot.config_tags.clone().unwrap_or_default();
        match build_source_snapshot_ops_candidate(&snapshot.path, &explicit_tags) {
            Ok(candidate) => candidates.push(candidate),
            Err(err) => {
                let folder_name = snapshot
                    .path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or("")
                    .to_string();
                if !folder_name.is_empty() {
                    candidates.push(SourceSnapshotOperationCandidate {
                        path: snapshot.path.display().to_string(),
                        folder_name,
                        source_modified_at: String::new(),
                        version_id: String::new(),
                        os: String::new(),
                        arch: String::new(),
                        snapshot_tags: manifest::SnapshotTagSet::default(),
                        disposition: SourceSnapshotOperationDisposition::Invalid,
                        error: Some(err.to_string()),
                    });
                }
            }
        }
    }

    candidates.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(candidates)
}

fn empty_repo_check_status(
    repo_path: String,
    exists: bool,
    error: Option<String>,
) -> RepoCheckStatus {
    RepoCheckStatus {
        exists,
        is_repo: false,
        repo_path,
        name: String::new(),
        remote_repo_id: String::new(),
        repo_manifest: None,
        snapshots: Vec::new(),
        bundle_count: 0,
        bundle_bytes: 0,
        artifact_count: 0,
        artifact_bytes: 0,
        error,
        timings: None,
    }
}

fn check_snapshot_manifests(
    repo_paths: &GutchRepoPaths,
    snapshots_root: &Path,
    available_artifacts: &HashSet<crate::ArtifactRef>,
) -> Result<(Vec<RepoCheckSnapshotStatus>, SnapshotManifestCheckTimings)> {
    if !snapshots_root.exists() {
        return Ok((Vec::new(), SnapshotManifestCheckTimings::default()));
    }

    let chunk_start = std::time::Instant::now();
    let chunk_index = load_chunk_index(repo_paths)?;
    let load_chunk_index_ms = chunk_start.elapsed().as_millis() as u64;

    let manifests_start = std::time::Instant::now();
    let mut snapshots = Vec::new();
    for snapshot_id in list_repo_snapshot_ids(repo_paths)? {
        let manifest_path = crate::snapshot_metadata_path(repo_paths, &snapshot_id)?;
        let manifest = load_snapshot_manifest(repo_paths, &snapshot_id)?;
        let snapshot_metadata =
            Some(serde_json::to_value(manifest.metadata()).map_err(|error| {
                Error::manifest(format!("failed to serialize snapshot metadata: {error}"))
            })?);
        let snapshot_product_metadata_hash_b3 =
            crate::serialize_snapshot_product_metadata_for_remote(&manifest)?
                .map(|bytes| crate::hash::hash_hex(&blake3::hash(&bytes)));
        let required_artifacts = crate::required_artifacts(&manifest, &chunk_index)?;
        let required_bundle_ids = required_bundle_ids(&manifest, &chunk_index)?;
        let missing_bundle_count = required_bundle_ids
            .iter()
            .filter(|bundle_id| {
                !available_artifacts.contains(&crate::ArtifactRef::Bundle {
                    id: (*bundle_id).clone(),
                })
            })
            .count();
        let missing_artifact_count = required_artifacts
            .iter()
            .filter(|artifact| !available_artifacts.contains(*artifact))
            .count();
        let launch_readiness_error =
            if manifest.product_metadata.is_some() && manifest.launch.is_none() {
                Some("No runtime target matches this host.".to_string())
            } else {
                None
            };

        let summary = summarize_snapshot_tags(&manifest.snapshot_tags);
        snapshots.push(RepoCheckSnapshotStatus {
            snapshot_id: manifest.snapshot_id.clone(),
            version_id: summary.version_id,
            os: summary.os,
            arch: summary.arch,
            file_count: manifest.file_count(),
            required_bundle_count: required_bundle_ids.len(),
            required_artifact_count: required_artifacts.len(),
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
    let load_snapshot_manifests_ms = manifests_start.elapsed().as_millis() as u64;

    Ok((
        snapshots,
        SnapshotManifestCheckTimings {
            load_chunk_index_ms,
            load_snapshot_manifests_ms,
        },
    ))
}

#[derive(Debug, Clone, Default)]
struct SnapshotManifestCheckTimings {
    load_chunk_index_ms: u64,
    load_snapshot_manifests_ms: u64,
}

pub(crate) fn build_source_snapshot_ops_candidate(
    path: &Path,
    explicit_tags: &manifest::SnapshotTagSet,
) -> Result<SourceSnapshotOperationCandidate> {
    let source_modified_at = source_modified_at_from_directory(path)?;
    build_source_snapshot_ops_candidate_with_modified_at(path, explicit_tags, source_modified_at)
}

pub(crate) fn build_source_snapshot_ops_candidate_with_modified_at(
    path: &Path,
    explicit_tags: &manifest::SnapshotTagSet,
    source_modified_at: String,
) -> Result<SourceSnapshotOperationCandidate> {
    use crate::snapshot::identity::resolve_snapshot_tags;

    let context = format!("source snapshot '{}'", path.display());
    let resolved = resolve_snapshot_tags(path, explicit_tags, &context)?;

    let folder_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            Error::manifest(format!("invalid source snapshot path: {}", path.display()))
        })?
        .to_string();

    let summary = summarize_snapshot_tags(&resolved.snapshot_tags);

    Ok(SourceSnapshotOperationCandidate {
        path: path.display().to_string(),
        folder_name,
        source_modified_at,
        version_id: summary.version_id,
        os: summary.os,
        arch: summary.arch,
        snapshot_tags: resolved.snapshot_tags,
        disposition: SourceSnapshotOperationDisposition::Unchanged,
        error: None,
    })
}

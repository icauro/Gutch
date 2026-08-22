use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::inventory::{list_repo_snapshot_ids, scan_bundle_inventory};
use super::snapshot_descriptor::{preview_snapshot_identity, summarize_snapshot_tags};
use super::{
    GutchRepoPaths, load_chunk_index, load_gutch_repo_manifest, load_snapshot_manifest,
    required_bundle_ids, snapshot_sources, source_modified_at_from_directory,
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
    pub product_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_tag_schema: Option<manifest::SnapshotTagSchema>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_manifest: Option<manifest::gutch_repo::GutchRepoManifest>,
    pub snapshots: Vec<RepoCheckSnapshotStatus>,
    #[serde(default)]
    pub bundle_count: usize,
    #[serde(default)]
    pub bundle_bytes: u64,
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timings: Option<RepoCheckTimings>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoCheckTimings {
    pub load_manifest_ms: u64,
    pub scan_bundles_ms: u64,
    pub load_chunk_index_ms: u64,
    pub load_snapshot_manifests_ms: u64,
    pub snapshot_count: usize,
    pub total_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackedRepoCheckStatus {
    pub repo_status: RepoCheckStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_sources_path: Option<String>,
    #[serde(default)]
    pub source_snapshot_operations: Vec<SourceSnapshotOperationCandidate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_sources_plan: Option<snapshot_sources::SnapshotSourcesSyncPlan>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timings: Option<TrackedRepoCheckTimings>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackedRepoCheckTimings {
    pub load_config_ms: u64,
    pub scan_sources_ms: u64,
    pub plan_sources_ms: u64,
    pub source_count: usize,
    pub total_ms: u64,
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
    pub total_logical_size: u64,
    pub manifest_path: String,
    pub missing_bundle_count: usize,
    pub source_modified_at: String,
    pub snapshot_tags: manifest::SnapshotTagSet,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_metadata: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_product_metadata_hash_b3: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch: Option<manifest::launch::ResolvedLaunchConfiguration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_launch: Option<manifest::launch::ResolvedLaunchConfiguration>,
    #[serde(default)]
    pub external_product_dependencies:
        Vec<manifest::external_dependency::ExternalProductDependencyRequirement>,
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
    pub snapshot_id: String,
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
    Create,
    Refresh,
    Invalid,
}

pub fn ensure_gutch_repo_tracked(repo_path: &str) -> Result<()> {
    let repo_path_str = repo_path.trim();
    if repo_path_str.is_empty() {
        return Ok(());
    }

    let mut config = crate::config::load_config()?;
    if config.find_repo(repo_path_str).is_none() {
        config.upsert_repo(crate::config::TrackedRepo {
            path: repo_path_str.to_string(),
            snapshot_sources_path: None,
        });
        crate::config::save_config(&config)?;
    }
    Ok(())
}

pub fn check_tracked_gutch_repo(repo_root: &Path) -> Result<TrackedRepoCheckStatus> {
    let total_start = std::time::Instant::now();
    let repo_status = check_gutch_repo(repo_root)?;
    if repo_status.is_repo {
        ensure_gutch_repo_tracked(&repo_status.repo_path)?;
    }

    let config_start = std::time::Instant::now();
    let snapshot_sources_path = crate::config::load_config()?
        .find_repo(&repo_status.repo_path)
        .and_then(|repo| repo.snapshot_sources_path.clone())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let load_config_ms = config_start.elapsed().as_millis() as u64;

    let sources_start = std::time::Instant::now();
    let mut source_snapshot_operations = match snapshot_sources_path.as_deref() {
        Some(path) if repo_status.is_repo => {
            discover_source_snapshot_operations(repo_root, Path::new(path))?
        }
        _ => Vec::new(),
    };
    let scan_sources_ms = sources_start.elapsed().as_millis() as u64;
    let source_count = source_snapshot_operations.len();

    let plan_start = std::time::Instant::now();
    let snapshot_sources_plan = if repo_status.is_repo {
        Some(snapshot_sources::plan_snapshot_sources_sync_with_context(
            repo_root,
            snapshot_sources_path.as_deref().map(Path::new),
            Some(source_snapshot_operations.as_slice()),
            Some(&repo_status),
        )?)
    } else {
        None
    };
    if let Some(plan) = snapshot_sources_plan.as_ref() {
        for candidate in &mut source_snapshot_operations {
            candidate.disposition = if candidate.error.is_some() {
                SourceSnapshotOperationDisposition::Invalid
            } else if plan
                .to_create
                .iter()
                .any(|entry| entry.path == candidate.path)
            {
                SourceSnapshotOperationDisposition::Create
            } else if plan
                .to_refresh
                .iter()
                .any(|entry| entry.path == candidate.path)
            {
                SourceSnapshotOperationDisposition::Refresh
            } else {
                SourceSnapshotOperationDisposition::Unchanged
            };
        }
    }
    let plan_sources_ms = plan_start.elapsed().as_millis() as u64;

    let total_ms = total_start.elapsed().as_millis() as u64;

    Ok(TrackedRepoCheckStatus {
        repo_status,
        snapshot_sources_path,
        source_snapshot_operations,
        snapshot_sources_plan,
        timings: Some(TrackedRepoCheckTimings {
            load_config_ms,
            scan_sources_ms,
            plan_sources_ms,
            source_count,
            total_ms,
        }),
    })
}

pub fn check_gutch_repo(repo_root: &Path) -> Result<RepoCheckStatus> {
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

    let scan_start = std::time::Instant::now();
    let (available_bundle_ids, bundle_count, bundle_bytes) =
        scan_bundle_inventory(&repo_paths.bundles_root)?;
    let scan_bundles_ms = scan_start.elapsed().as_millis() as u64;

    let (snapshots, snapshot_check_timings) = check_snapshot_manifests(
        &repo_paths,
        &repo_paths.snapshots_root,
        &available_bundle_ids,
    )?;
    let snapshot_count = snapshots.len();

    let total_ms = total_start.elapsed().as_millis() as u64;

    Ok(RepoCheckStatus {
        exists: true,
        is_repo: true,
        repo_path: repo_path_display,
        name: repo_manifest.name.clone(),
        remote_repo_id: repo_manifest.remote_repo_id.clone(),
        product_id: repo_manifest.product_id.clone(),
        snapshot_tag_schema: repo_manifest.snapshot_tag_schema.clone(),
        repo_manifest: Some(repo_manifest),
        snapshots,
        bundle_count,
        bundle_bytes,
        error: None,
        timings: Some(RepoCheckTimings {
            load_manifest_ms,
            scan_bundles_ms,
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

    let (_repo_paths, repo_manifest) = load_gutch_repo_manifest(repo_root)?;
    let rules = repo_manifest.snapshot_tag_schema.unwrap_or_default();
    let config =
        crate::snapshot::source_config::SourceConfig::load_or_default(source_snapshots_root)?;
    let mut candidates = Vec::new();

    for snapshot in config.discover_snapshot_sources(source_snapshots_root)? {
        let explicit_tags = snapshot.config_tags.clone().unwrap_or_default();
        match build_source_snapshot_ops_candidate(
            &snapshot.path,
            &rules,
            &explicit_tags,
            snapshot.allow_name_inference,
        ) {
            Ok(candidate) => candidates.push(candidate),
            Err(err) => {
                if !snapshot.allow_name_inference {
                    // A declared explicit snapshot that cannot resolve its tags is
                    // a hard error rather than a skipped folder.
                    return Err(err);
                }
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
                        snapshot_id: String::new(),
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

pub fn preview_source_snapshot_ops(
    repo_root: &Path,
    source_dir: &Path,
) -> Result<SourceSnapshotOperationCandidate> {
    if !source_dir.exists() {
        return Err(Error::manifest(format!(
            "source snapshots path does not exist: {}",
            source_dir.display()
        )));
    }
    if !source_dir.is_dir() {
        return Err(Error::manifest(format!(
            "source snapshots path is not a directory: {}",
            source_dir.display()
        )));
    }

    let (_repo_paths, repo_manifest) = load_gutch_repo_manifest(repo_root)?;
    let rules = repo_manifest.snapshot_tag_schema.unwrap_or_default();
    build_source_snapshot_ops_candidate(
        source_dir,
        &rules,
        &manifest::SnapshotTagSet::default(),
        true,
    )
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
        product_id: String::new(),
        snapshot_tag_schema: None,
        repo_manifest: None,
        snapshots: Vec::new(),
        bundle_count: 0,
        bundle_bytes: 0,
        error,
        timings: None,
    }
}

fn check_snapshot_manifests(
    repo_paths: &GutchRepoPaths,
    snapshots_root: &Path,
    available_bundle_ids: &HashSet<String>,
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
        let manifest_path = crate::snapshot_metadata_path(repo_paths, &snapshot_id);
        let manifest = load_snapshot_manifest(repo_paths, &snapshot_id)?;
        let snapshot_metadata =
            Some(serde_json::to_value(manifest.metadata()).map_err(|error| {
                Error::manifest(format!("failed to serialize snapshot metadata: {error}"))
            })?);
        let snapshot_product_metadata_hash_b3 =
            crate::serialize_snapshot_product_metadata_for_remote(&manifest)?
                .map(|bytes| crate::hash::hash_hex(&blake3::hash(&bytes)));
        let required_bundle_ids = required_bundle_ids(&manifest, &chunk_index);
        let missing_bundle_count = required_bundle_ids
            .iter()
            .filter(|bundle_id| !available_bundle_ids.contains(*bundle_id))
            .count();
        let resolved_launch = if let Some(launch) = manifest.launch.as_ref() {
            manifest::launch::validate_resolved_launch_for_snapshot(launch, &manifest)?;
            Some(launch.clone())
        } else {
            None
        };
        let external_product_dependencies = manifest
            .product_metadata
            .as_ref()
            .map(|metadata| {
                metadata.external_dependencies_for_host_tags(&manifest::current_host_tags())
            })
            .unwrap_or_default();

        let summary = summarize_snapshot_tags(&manifest.snapshot_tags, None);
        snapshots.push(RepoCheckSnapshotStatus {
            snapshot_id: manifest.snapshot_id.clone(),
            version_id: summary.version_id,
            os: summary.os,
            arch: summary.arch,
            file_count: manifest.file_count(),
            required_bundle_count: required_bundle_ids.len(),
            total_logical_size: manifest.total_logical_size,
            manifest_path: manifest_path.display().to_string(),
            missing_bundle_count,
            source_modified_at: manifest.source_modified_at.clone(),
            snapshot_tags: manifest.snapshot_tags.clone(),
            snapshot_metadata,
            snapshot_product_metadata_hash_b3,
            launch: manifest.launch.clone(),
            resolved_launch,
            external_product_dependencies,
            file_paths: manifest.files().map(|file| file.path.clone()).collect(),
            launch_readiness_error: None,
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
    rules: &manifest::SnapshotTagSchema,
    explicit_tags: &manifest::SnapshotTagSet,
    allow_name_inference: bool,
) -> Result<SourceSnapshotOperationCandidate> {
    use crate::core::snapshot_identity::resolve_snapshot_identity_with_inference;

    let context = format!("source snapshot '{}'", path.display());
    let resolved = resolve_snapshot_identity_with_inference(
        path,
        explicit_tags,
        rules,
        allow_name_inference,
        &context,
    )?;

    let folder_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            Error::manifest(format!("invalid source snapshot path: {}", path.display()))
        })?
        .to_string();

    let summary = summarize_snapshot_tags(&resolved.snapshot_tags, Some(rules));

    Ok(SourceSnapshotOperationCandidate {
        path: path.display().to_string(),
        folder_name,
        snapshot_id: preview_snapshot_identity(&resolved.snapshot_tags, Some(rules)),
        source_modified_at: source_modified_at_from_directory(path)?,
        version_id: summary.version_id,
        os: summary.os,
        arch: summary.arch,
        snapshot_tags: resolved.snapshot_tags,
        disposition: SourceSnapshotOperationDisposition::Unchanged,
        error: None,
    })
}

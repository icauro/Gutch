use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::inventory::{list_repo_target_ids, scan_bundle_inventory};
use super::target_descriptor::{preview_target_identity, summarize_target_tags};
use super::{
    GutchRepoPaths, load_chunk_index, load_gutch_repo_manifest, load_target_manifest,
    required_bundle_ids, source_modified_at_from_directory, target_sources,
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
    pub target_schema: Option<manifest::TargetTagSchema>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_manifest: Option<manifest::gutch_repo::GutchRepoManifest>,
    pub targets: Vec<RepoCheckTargetStatus>,
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
    pub load_target_manifests_ms: u64,
    pub target_count: usize,
    pub total_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackedRepoCheckStatus {
    pub repo_status: RepoCheckStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_sources_path: Option<String>,
    #[serde(default)]
    pub source_target_installs: Vec<SourceTargetInstallCandidate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_sources_plan: Option<target_sources::TargetSourcesSyncPlan>,
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
pub struct RepoCheckTargetStatus {
    pub target_id: String,
    pub version_id: String,
    pub os: String,
    pub arch: String,
    pub file_count: usize,
    pub required_bundle_count: usize,
    pub total_logical_size: u64,
    pub manifest_path: String,
    pub missing_bundle_count: usize,
    pub source_modified_at: String,
    pub target_tags: manifest::TargetTagSet,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_metadata: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch: Option<manifest::launch::ResolvedLaunchConfiguration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_launch: Option<manifest::launch::ResolvedLaunchConfiguration>,
    #[serde(default)]
    pub external_product_dependencies:
        Vec<manifest::external_dependency::ExternalProductDependencyRequirement>,
    #[serde(default)]
    pub file_paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceTargetInstallCandidate {
    pub path: String,
    pub folder_name: String,
    pub target_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub content_target_id: String,
    #[serde(default)]
    pub source_modified_at: String,
    pub version_id: String,
    pub os: String,
    pub arch: String,
    pub target_tags: manifest::TargetTagSet,
    #[serde(default)]
    pub disposition: SourceTargetInstallDisposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceTargetInstallDisposition {
    #[default]
    Unchanged,
    Insert,
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
            target_sources_path: None,
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
    let target_sources_path = crate::config::load_config()?
        .find_repo(&repo_status.repo_path)
        .and_then(|repo| repo.target_sources_path.clone())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let load_config_ms = config_start.elapsed().as_millis() as u64;

    let sources_start = std::time::Instant::now();
    let mut source_target_installs = match target_sources_path.as_deref() {
        Some(path) if repo_status.is_repo => {
            discover_source_target_installs(repo_root, Path::new(path))?
        }
        _ => Vec::new(),
    };
    let scan_sources_ms = sources_start.elapsed().as_millis() as u64;
    let source_count = source_target_installs.len();

    let plan_start = std::time::Instant::now();
    let target_sources_plan = if repo_status.is_repo {
        Some(target_sources::plan_target_sources_sync_with_context(
            repo_root,
            target_sources_path.as_deref().map(Path::new),
            Some(source_target_installs.as_slice()),
            Some(&repo_status),
        )?)
    } else {
        None
    };
    if let Some(plan) = target_sources_plan.as_ref() {
        for candidate in &mut source_target_installs {
            candidate.disposition = if candidate.error.is_some() {
                SourceTargetInstallDisposition::Invalid
            } else if plan
                .to_insert
                .iter()
                .any(|entry| entry.path == candidate.path)
            {
                SourceTargetInstallDisposition::Insert
            } else if plan
                .to_refresh
                .iter()
                .any(|entry| entry.path == candidate.path)
            {
                SourceTargetInstallDisposition::Refresh
            } else {
                SourceTargetInstallDisposition::Unchanged
            };
        }
    }
    let plan_sources_ms = plan_start.elapsed().as_millis() as u64;

    let total_ms = total_start.elapsed().as_millis() as u64;

    Ok(TrackedRepoCheckStatus {
        repo_status,
        target_sources_path,
        source_target_installs,
        target_sources_plan,
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

    let (targets, target_check_timings) =
        check_target_manifests(&repo_paths, &repo_paths.targets_root, &available_bundle_ids)?;
    let target_count = targets.len();

    let total_ms = total_start.elapsed().as_millis() as u64;

    Ok(RepoCheckStatus {
        exists: true,
        is_repo: true,
        repo_path: repo_path_display,
        name: repo_manifest.name.clone(),
        remote_repo_id: repo_manifest.remote_repo_id.clone(),
        product_id: repo_manifest.product_id.clone(),
        target_schema: repo_manifest.target_schema.clone(),
        repo_manifest: Some(repo_manifest),
        targets,
        bundle_count,
        bundle_bytes,
        error: None,
        timings: Some(RepoCheckTimings {
            load_manifest_ms,
            scan_bundles_ms,
            load_chunk_index_ms: target_check_timings.load_chunk_index_ms,
            load_target_manifests_ms: target_check_timings.load_target_manifests_ms,
            target_count,
            total_ms,
        }),
    })
}

pub fn discover_source_target_installs(
    repo_root: &Path,
    source_targets_root: &Path,
) -> Result<Vec<SourceTargetInstallCandidate>> {
    if !source_targets_root.exists() {
        return Err(Error::manifest(format!(
            "source targets path does not exist: {}",
            source_targets_root.display()
        )));
    }
    if !source_targets_root.is_dir() {
        return Err(Error::manifest(format!(
            "source targets path is not a directory: {}",
            source_targets_root.display()
        )));
    }

    let (_repo_paths, repo_manifest) = load_gutch_repo_manifest(repo_root)?;
    let rules = repo_manifest.target_schema.unwrap_or_default();
    let config = crate::target::source_config::SourceConfig::load_or_default(source_targets_root)?;
    let mut candidates = Vec::new();

    for target in config.discover_target_sources(source_targets_root)? {
        let explicit_tags = target.config_tags.clone().unwrap_or_default();
        match build_source_target_install_candidate(
            &target.path,
            &rules,
            &explicit_tags,
            target.allow_name_inference,
        ) {
            Ok(candidate) => candidates.push(candidate),
            Err(err) => {
                if !target.allow_name_inference {
                    // A declared explicit target that cannot resolve its tags is
                    // a hard error rather than a skipped folder.
                    return Err(err);
                }
                let folder_name = target
                    .path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or("")
                    .to_string();
                if !folder_name.is_empty() {
                    candidates.push(SourceTargetInstallCandidate {
                        path: target.path.display().to_string(),
                        folder_name,
                        target_id: String::new(),
                        content_target_id: String::new(),
                        source_modified_at: String::new(),
                        version_id: String::new(),
                        os: String::new(),
                        arch: String::new(),
                        target_tags: manifest::TargetTagSet::default(),
                        disposition: SourceTargetInstallDisposition::Invalid,
                        error: Some(err.to_string()),
                    });
                }
            }
        }
    }

    candidates.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(candidates)
}

pub fn preview_source_target_install(
    repo_root: &Path,
    source_dir: &Path,
) -> Result<SourceTargetInstallCandidate> {
    if !source_dir.exists() {
        return Err(Error::manifest(format!(
            "source targets path does not exist: {}",
            source_dir.display()
        )));
    }
    if !source_dir.is_dir() {
        return Err(Error::manifest(format!(
            "source targets path is not a directory: {}",
            source_dir.display()
        )));
    }

    let (_repo_paths, repo_manifest) = load_gutch_repo_manifest(repo_root)?;
    let rules = repo_manifest.target_schema.unwrap_or_default();
    build_source_target_install_candidate(
        source_dir,
        &rules,
        &manifest::TargetTagSet::default(),
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
        target_schema: None,
        repo_manifest: None,
        targets: Vec::new(),
        bundle_count: 0,
        bundle_bytes: 0,
        error,
        timings: None,
    }
}

fn check_target_manifests(
    repo_paths: &GutchRepoPaths,
    targets_root: &Path,
    available_bundle_ids: &HashSet<String>,
) -> Result<(Vec<RepoCheckTargetStatus>, TargetManifestCheckTimings)> {
    if !targets_root.exists() {
        return Ok((Vec::new(), TargetManifestCheckTimings::default()));
    }

    let chunk_start = std::time::Instant::now();
    let chunk_index = load_chunk_index(repo_paths)?;
    let load_chunk_index_ms = chunk_start.elapsed().as_millis() as u64;

    let manifests_start = std::time::Instant::now();
    let mut targets = Vec::new();
    for target_id in list_repo_target_ids(repo_paths)? {
        let manifest_path = crate::target_manifest_path(repo_paths, &target_id);
        let manifest = load_target_manifest(repo_paths, &target_id)?;
        let target_metadata = Some(
            serde_json::to_value(&manifest).map_err(|error| {
                Error::manifest(format!("failed to serialize target metadata: {error}"))
            })?,
        );
        let required_bundle_ids = required_bundle_ids(&manifest, &chunk_index);
        let missing_bundle_count = required_bundle_ids
            .iter()
            .filter(|bundle_id| !available_bundle_ids.contains(*bundle_id))
            .count();
        let resolved_launch = if let Some(launch) = manifest.launch.as_ref() {
            manifest::launch::validate_resolved_launch_for_target(launch, &manifest)?;
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

        let summary = summarize_target_tags(&manifest.target_tags, None);
        targets.push(RepoCheckTargetStatus {
            target_id: manifest.target_id.clone(),
            version_id: summary.version_id,
            os: summary.os,
            arch: summary.arch,
            file_count: manifest.files.len(),
            required_bundle_count: required_bundle_ids.len(),
            total_logical_size: manifest.total_logical_size,
            manifest_path: manifest_path.display().to_string(),
            missing_bundle_count,
            source_modified_at: manifest.source_modified_at.clone(),
            target_tags: manifest.target_tags,
            target_metadata,
            launch: manifest.launch,
            resolved_launch,
            external_product_dependencies,
            file_paths: manifest.files.into_iter().map(|file| file.path).collect(),
        });
    }

    targets.sort_by(|left, right| left.target_id.cmp(&right.target_id));
    let load_target_manifests_ms = manifests_start.elapsed().as_millis() as u64;

    Ok((
        targets,
        TargetManifestCheckTimings {
            load_chunk_index_ms,
            load_target_manifests_ms,
        },
    ))
}

#[derive(Debug, Clone, Default)]
struct TargetManifestCheckTimings {
    load_chunk_index_ms: u64,
    load_target_manifests_ms: u64,
}

pub(crate) fn build_source_target_install_candidate(
    path: &Path,
    rules: &manifest::TargetTagSchema,
    explicit_tags: &manifest::TargetTagSet,
    allow_name_inference: bool,
) -> Result<SourceTargetInstallCandidate> {
    use crate::core::target_identity::resolve_target_identity_with_inference;

    let context = format!("source target '{}'", path.display());
    let resolved = resolve_target_identity_with_inference(
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
        .ok_or_else(|| Error::manifest(format!("invalid source target path: {}", path.display())))?
        .to_string();

    let summary = summarize_target_tags(&resolved.target_tags, Some(rules));

    Ok(SourceTargetInstallCandidate {
        path: path.display().to_string(),
        folder_name,
        target_id: preview_target_identity(&resolved.target_tags, Some(rules)),
        content_target_id: String::new(),
        source_modified_at: source_modified_at_from_directory(path)?,
        version_id: summary.version_id,
        os: summary.os,
        arch: summary.arch,
        target_tags: resolved.target_tags,
        disposition: SourceTargetInstallDisposition::Unchanged,
        error: None,
    })
}

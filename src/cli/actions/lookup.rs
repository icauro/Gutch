use std::path::Path;

use serde::Serialize;

use crate::cli::TargetCommands;
use gutch as g;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LookupTargetApplyResult {
    pub inserted_count: usize,
    pub removed_count: usize,
}

pub fn insert_uninserted_lookup_installs(
    repo: &Path,
    lookup_path: &Path,
) -> g::Result<LookupTargetApplyResult> {
    let plan = g::plan_lookup_target_changes(repo, Some(lookup_path))?;
    if plan.to_insert.is_empty() {
        return Ok(LookupTargetApplyResult {
            inserted_count: 0,
            removed_count: 0,
        });
    }

    let (_, repo_manifest) = g::load_gutch_repo_manifest(repo)?;
    let compression_profile = repo_manifest.compression_profile.preset;
    let mut inserted_count = 0;

    for candidate in &plan.to_insert {
        crate::cli::target::run(TargetCommands::Insert {
            dir: Path::new(&candidate.path).to_path_buf(),
            repo: repo.to_path_buf(),
            version: Some(candidate.version_id.clone()),
            platform: Some(candidate.platform.clone()),
            architecture: Some(candidate.architecture.clone()),
            tier: Some(candidate.tier_id.clone()),
            tags: Vec::new(),
            chunk_profile: g::manifest::ArtifactSizingPreset::Chunk128KiB,
            bundle_profile: g::manifest::BundleSizingPreset::Bundle16MiB,
            compression_profile,
            storage_mode: g::manifest::ArtifactStorageMode::Bundled,
            memory_mode: g::MemoryMode::Auto,
            max_memory_mib: None,
            jobs: None,
            summary_out: None,
            stats_json_out: None,
        })?;
        inserted_count += 1;
    }

    Ok(LookupTargetApplyResult {
        inserted_count,
        removed_count: 0,
    })
}

pub fn remove_lookup_missing_targets(
    repo: &Path,
    lookup_path: &Path,
) -> g::Result<LookupTargetApplyResult> {
    let plan = g::plan_lookup_target_changes(repo, Some(lookup_path))?;
    if plan.to_remove.is_empty() {
        return Ok(LookupTargetApplyResult {
            inserted_count: 0,
            removed_count: 0,
        });
    }

    let mut removed_count = 0;
    for target_id in &plan.to_remove {
        crate::cli::target::run(TargetCommands::Delete {
            repo: repo.to_path_buf(),
            target: target_id.clone(),
            yes: true,
        })?;
        removed_count += 1;
    }

    Ok(LookupTargetApplyResult {
        inserted_count: 0,
        removed_count,
    })
}

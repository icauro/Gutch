use std::path::Path;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::artifact::bundle;
use crate::verify::VerifyMode;
use crate::{Error, Result};

use super::{
    GutchRepoPaths, bundle_path, check_gutch_repo, load_chunk_index, load_gutch_repo_manifest,
    load_target_manifest, required_bundle_ids,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoVerifyResult {
    pub ok: bool,
    pub mode: String,
    pub target_count: usize,
    pub targets_ok: usize,
    pub targets: Vec<TargetVerifyResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timings: Option<RepoVerifyTimings>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoVerifyTimings {
    pub load_ms: u64,
    pub verify_bundles_ms: u64,
    pub total_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetVerifyResult {
    pub target_id: String,
    pub ok: bool,
    pub check_passed: bool,
    pub required_bundle_count: usize,
    pub missing_bundle_count: usize,
    pub missing_bundles: Vec<String>,
    pub corrupt_bundle_count: usize,
    pub corrupt_bundles: Vec<String>,
}

/// Verify repo bundle integrity. When `target_id` is None, all repo targets are verified.
pub fn verify_gutch_repo(
    repo_root: &Path,
    target_id: Option<&str>,
    mode: VerifyMode,
) -> Result<RepoVerifyResult> {
    let total_start = Instant::now();
    let check = check_gutch_repo(repo_root)?;
    if !check.is_repo {
        return Err(Error::manifest(format!(
            "no gutch repo found at {}",
            repo_root.display()
        )));
    }

    let load_start = Instant::now();
    let (repo_paths, _) = load_gutch_repo_manifest(repo_root)?;
    let chunk_index = load_chunk_index(&repo_paths)?;
    let load_ms = load_start.elapsed().as_millis() as u64;

    let selected_targets: Vec<_> = check
        .targets
        .iter()
        .filter(|target| target_id.map(|id| id == target.target_id).unwrap_or(true))
        .collect();

    if let Some(id) = target_id {
        if selected_targets.is_empty() {
            return Err(Error::manifest(format!("target not found in repo: {id}")));
        }
    }

    let verify_start = Instant::now();
    let mut targets = Vec::new();
    for target in selected_targets {
        targets.push(verify_target_bundles(
            &repo_paths,
            &chunk_index,
            &target.target_id,
            mode,
        )?);
    }
    let verify_bundles_ms = verify_start.elapsed().as_millis() as u64;

    let targets_ok = targets.iter().filter(|target| target.ok).count();
    let ok = targets_ok == targets.len();

    Ok(RepoVerifyResult {
        ok,
        mode: mode.as_str().to_string(),
        target_count: targets.len(),
        targets_ok,
        targets,
        timings: Some(RepoVerifyTimings {
            load_ms,
            verify_bundles_ms,
            total_ms: total_start.elapsed().as_millis() as u64,
        }),
    })
}

fn verify_target_bundles(
    repo_paths: &GutchRepoPaths,
    chunk_index: &crate::manifest::ChunkIndex,
    target_id: &str,
    mode: VerifyMode,
) -> Result<TargetVerifyResult> {
    let target_manifest = load_target_manifest(repo_paths, target_id)?;
    let required = required_bundle_ids(&target_manifest, chunk_index);

    let mut missing = Vec::new();
    let mut corrupt = Vec::new();

    for bundle_id in &required {
        let path = bundle_path(repo_paths, bundle_id);
        if !path.exists() {
            missing.push(bundle_id.clone());
            continue;
        }
        if let Err(err) = verify_bundle_file(&path, mode) {
            corrupt.push(format!("{bundle_id}: {err}"));
        }
    }

    let check_passed = missing.is_empty();
    let ok = check_passed && corrupt.is_empty();

    Ok(TargetVerifyResult {
        target_id: target_id.to_string(),
        ok,
        check_passed,
        required_bundle_count: required.len(),
        missing_bundle_count: missing.len(),
        missing_bundles: missing,
        corrupt_bundle_count: corrupt.len(),
        corrupt_bundles: corrupt,
    })
}

fn verify_bundle_file(path: &Path, mode: VerifyMode) -> Result<()> {
    let data = std::fs::read(path)?;
    let mut reader = bundle::read_bundle(&data)?;
    if mode == VerifyMode::Strict {
        reader.verify()?;
    }
    Ok(())
}

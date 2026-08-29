use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::artifact::bundle;
use crate::verify::VerifyMode;
use crate::{Error, Result};

use super::{
    GutchRepoPaths, bundle_path, check_gutch_repo, load_chunk_index, load_gutch_repo_manifest,
    load_snapshot_manifest, required_bundle_ids,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoVerifyResult {
    pub ok: bool,
    pub mode: String,
    pub snapshot_count: usize,
    pub snapshots_ok: usize,
    pub snapshots: Vec<SnapshotVerifyResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotVerifyResult {
    pub snapshot_id: String,
    pub ok: bool,
    pub check_passed: bool,
    pub required_bundle_count: usize,
    pub missing_bundle_count: usize,
    pub missing_bundles: Vec<String>,
    pub corrupt_bundle_count: usize,
    pub corrupt_bundles: Vec<String>,
}

/// Verify repo bundle integrity. When `snapshot_id` is None, all repo snapshots are verified.
pub fn verify_gutch_repo(
    repo_root: &Path,
    snapshot_id: Option<&str>,
    mode: VerifyMode,
) -> Result<RepoVerifyResult> {
    let check = check_gutch_repo(repo_root)?;
    if !check.is_repo {
        return Err(Error::manifest(format!(
            "no gutch repo found at {}",
            repo_root.display()
        )));
    }

    let (repo_paths, repo_manifest) = load_gutch_repo_manifest(repo_root)?;
    let chunk_index = load_chunk_index(&repo_paths)?;

    let selected_snapshots: Vec<_> = check
        .snapshots
        .iter()
        .filter(|snapshot| {
            snapshot_id
                .map(|id| id == snapshot.snapshot_id)
                .unwrap_or(true)
        })
        .collect();

    if let Some(id) = snapshot_id
        && selected_snapshots.is_empty()
    {
        return Err(Error::manifest(format!("snapshot not found in repo: {id}")));
    }

    let mut snapshots = Vec::new();
    for snapshot in selected_snapshots {
        snapshots.push(verify_snapshot_bundles(
            &repo_paths,
            &chunk_index,
            &repo_manifest.compression_profile(),
            &snapshot.snapshot_id,
            mode,
        )?);
    }
    let snapshots_ok = snapshots.iter().filter(|snapshot| snapshot.ok).count();
    let ok = snapshots_ok == snapshots.len();

    Ok(RepoVerifyResult {
        ok,
        mode: mode.as_str().to_string(),
        snapshot_count: snapshots.len(),
        snapshots_ok,
        snapshots,
    })
}

fn verify_snapshot_bundles(
    repo_paths: &GutchRepoPaths,
    chunk_index: &crate::manifest::ChunkIndex,
    compression_profile: &crate::manifest::CompressionProfile,
    snapshot_id: &str,
    mode: VerifyMode,
) -> Result<SnapshotVerifyResult> {
    let snapshot_manifest = load_snapshot_manifest(repo_paths, snapshot_id)?;
    let required = required_bundle_ids(&snapshot_manifest, chunk_index);

    let mut missing = Vec::new();
    let mut corrupt = Vec::new();

    for bundle_id in &required {
        let path = bundle_path(repo_paths, bundle_id)?;
        if !path.exists() {
            missing.push(bundle_id.clone());
            continue;
        }
        if let Err(err) = verify_bundle_file(&path, compression_profile, mode) {
            corrupt.push(format!("{bundle_id}: {err}"));
        }
    }

    let check_passed = missing.is_empty();
    let ok = check_passed && corrupt.is_empty();

    Ok(SnapshotVerifyResult {
        snapshot_id: snapshot_id.to_string(),
        ok,
        check_passed,
        required_bundle_count: required.len(),
        missing_bundle_count: missing.len(),
        missing_bundles: missing,
        corrupt_bundle_count: corrupt.len(),
        corrupt_bundles: corrupt,
    })
}

fn verify_bundle_file(
    path: &Path,
    compression_profile: &crate::manifest::CompressionProfile,
    mode: VerifyMode,
) -> Result<()> {
    let data = std::fs::read(path)?;
    let mut reader = bundle::read_bundle(&data, compression_profile)?;
    if mode == VerifyMode::Strict {
        reader.verify()?;
    }
    Ok(())
}

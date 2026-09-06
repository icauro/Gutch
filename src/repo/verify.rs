use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::artifact::bundle;
use crate::artifact::store::ArtifactRef;
use crate::verify::VerifyMode;
use crate::{Error, Result};

use super::{
    GutchRepoPaths, check_gutch_repo, load_chunk_index, load_gutch_repo_manifest,
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
    pub required_artifact_count: usize,
    pub missing_bundle_count: usize,
    pub missing_bundles: Vec<String>,
    pub missing_artifact_count: usize,
    pub missing_artifacts: Vec<String>,
    pub corrupt_bundle_count: usize,
    pub corrupt_bundles: Vec<String>,
    pub corrupt_artifact_count: usize,
    pub corrupt_artifacts: Vec<String>,
}

/// Verify repo artifact integrity. When `snapshot_id` is None, all repo snapshots are verified.
pub fn verify_gutch_repo_artifacts(
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
        snapshots.push(verify_snapshot_artifacts(
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

pub(crate) fn verify_snapshot_artifacts(
    repo_paths: &GutchRepoPaths,
    chunk_index: &crate::manifest::ChunkIndex,
    compression_profile: &crate::manifest::CompressionProfile,
    snapshot_id: &str,
    mode: VerifyMode,
) -> Result<SnapshotVerifyResult> {
    let snapshot_manifest = load_snapshot_manifest(repo_paths, snapshot_id)?;
    let required_artifacts = crate::required_artifacts(&snapshot_manifest, chunk_index)?;
    let required_bundles = required_bundle_ids(&snapshot_manifest, chunk_index)?;

    let mut missing_bundles = Vec::new();
    let mut missing_artifacts = Vec::new();
    let mut corrupt_bundles = Vec::new();
    let mut corrupt_artifacts = Vec::new();

    for artifact in &required_artifacts {
        let path = artifact.local_path(repo_paths)?;
        if !path.exists() {
            if let ArtifactRef::Bundle { id } = artifact {
                missing_bundles.push(id.clone());
            }
            missing_artifacts.push(artifact.id().to_string());
            continue;
        }
        let result = match artifact {
            ArtifactRef::Bundle { id } => verify_bundle_file(&path, id, compression_profile, mode),
            ArtifactRef::LooseChunk { hash } => {
                let entry = chunk_index.entries.get(hash).ok_or_else(|| {
                    Error::manifest(format!(
                        "missing chunk index entry for required chunk '{hash}'"
                    ))
                });
                entry.and_then(|entry| {
                    crate::artifact::store::read_loose_chunk(
                        &path,
                        hash,
                        entry.raw_size,
                        compression_profile,
                    )
                    .map(|_| ())
                })
            }
        };
        if let Err(err) = result {
            if let ArtifactRef::Bundle { id } = artifact {
                corrupt_bundles.push(format!("{id}: {err}"));
            }
            corrupt_artifacts.push(format!("{}: {err}", artifact.id()));
        }
    }

    let check_passed = missing_artifacts.is_empty();
    let ok = check_passed && corrupt_artifacts.is_empty();

    Ok(SnapshotVerifyResult {
        snapshot_id: snapshot_id.to_string(),
        ok,
        check_passed,
        required_bundle_count: required_bundles.len(),
        required_artifact_count: required_artifacts.len(),
        missing_bundle_count: missing_bundles.len(),
        missing_bundles,
        missing_artifact_count: missing_artifacts.len(),
        missing_artifacts,
        corrupt_bundle_count: corrupt_bundles.len(),
        corrupt_bundles,
        corrupt_artifact_count: corrupt_artifacts.len(),
        corrupt_artifacts,
    })
}

fn verify_bundle_file(
    path: &Path,
    bundle_id: &str,
    compression_profile: &crate::manifest::CompressionProfile,
    mode: VerifyMode,
) -> Result<()> {
    let mut reader = bundle::BundleReader::open(std::fs::File::open(path)?, compression_profile)?;
    if mode == VerifyMode::Strict {
        bundle::verify_bundle_identity_file(path, bundle_id)?;
        reader.verify()?;
    }
    Ok(())
}

//! Repository artifact verification by structure or identity.

use serde::Deserialize;
use serde::Serialize;
use std::path::Path;

use crate::Error;
use crate::Result;
use crate::artifact::bundle;
use crate::artifact::reference::ArtifactRef;
use crate::manifest::SnapshotManifest;

use super::inventory::list_repo_snapshot_ids;
use crate::artifact::loose::read_loose_chunk;
use crate::repo::artifacts::RequiredArtifacts;
use crate::repo::artifacts::classify_required_artifacts;
use crate::repo::artifacts::required_artifacts;
use crate::repo::coordination;
use crate::repo::index::ChunkIndex;
use crate::repo::index::load_chunk_index;
use crate::repo::paths::RepoPaths;
use crate::repo::paths::load_repo_manifest;
use crate::repo::snapshot_manifests::load_snapshot_manifest;
use std::collections::HashMap;
use std::fs::File;

/// `Fast` checks artifact structure; `Strict` also checks content-addressed identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepoArtifactVerification {
    Fast,
    Strict,
}
impl RepoArtifactVerification {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fast => "fast",
            Self::Strict => "strict",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoArtifactVerifyResult {
    pub ok: bool,
    pub mode: RepoArtifactVerification,
    pub snapshot_count: usize,
    pub snapshots_ok: usize,
    pub snapshots: Vec<SnapshotArtifactVerifyResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotArtifactVerifyResult {
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
pub fn verify_repo(
    repo_root: &Path,
    snapshot_id: Option<&str>,
    mode: RepoArtifactVerification,
) -> Result<RepoArtifactVerifyResult> {
    coordination::read(repo_root, || {
        let (repo_paths, repo_manifest) = load_repo_manifest(repo_root)?;
        let chunk_index = load_chunk_index(&repo_paths)?;
        let constraints = repo_manifest.constraints.clone().unwrap_or_default();

        let mut snapshots = Vec::new();
        let mut verified = HashMap::new();
        for stored_snapshot_id in list_repo_snapshot_ids(&repo_paths)? {
            let manifest = load_snapshot_manifest(&repo_paths, &stored_snapshot_id)?;
            if snapshot_id.is_some_and(|id| id != stored_snapshot_id) {
                // Snapshot selection narrows payload checks, not repository metadata validation.
                required_artifacts(&manifest, &chunk_index)?;
                continue;
            }
            constraints.validate_snapshot_paths(&manifest, "verify")?;
            snapshots.push(verify_snapshot_artifacts(
                &repo_paths,
                &chunk_index,
                &manifest,
                mode,
                &mut verified,
            )?);
        }
        if let Some(id) = snapshot_id
            && snapshots.is_empty()
        {
            return Err(Error::repository(format!(
                "snapshot not found in repo: {id}"
            )));
        }
        let snapshots_ok = snapshots.iter().filter(|snapshot| snapshot.ok).count();
        let ok = snapshots_ok == snapshots.len();

        Ok(RepoArtifactVerifyResult {
            ok,
            mode,
            snapshot_count: snapshots.len(),
            snapshots_ok,
            snapshots,
        })
    })
}

/// Artifact check outcomes shared across snapshots, so each artifact is read once.
pub(crate) type VerifiedArtifacts = HashMap<ArtifactRef, std::result::Result<(), String>>;

/// Verifies an already loaded snapshot manifest, reusing earlier artifact outcomes.
pub(crate) fn verify_snapshot_artifacts(
    repo_paths: &RepoPaths,
    chunk_index: &ChunkIndex,
    snapshot_manifest: &SnapshotManifest,
    mode: RepoArtifactVerification,
    verified: &mut VerifiedArtifacts,
) -> Result<SnapshotArtifactVerifyResult> {
    let RequiredArtifacts {
        artifacts: required_artifacts,
        unindexed_chunks,
    } = classify_required_artifacts(snapshot_manifest, chunk_index)?;
    let required_bundle_count = required_artifacts
        .iter()
        .filter(|artifact| artifact.is_bundle())
        .count();

    let mut missing_bundles = Vec::new();
    // Unindexed chunks cannot be located in any artifact; report them by hash.
    let mut missing_artifacts = unindexed_chunks
        .iter()
        .map(|hash| format!("unindexed chunk {hash}"))
        .collect::<Vec<_>>();
    let mut corrupt_bundles = Vec::new();
    let mut corrupt_artifacts = Vec::new();

    for artifact in &required_artifacts {
        let path = artifact.local_path(repo_paths)?;
        if !path.exists() {
            if let ArtifactRef::Bundle { id } = artifact {
                missing_bundles.push(id.to_string());
            }
            missing_artifacts.push(artifact.id().to_string());
            continue;
        }
        let result = if let Some(result) = verified.get(artifact) {
            result.clone()
        } else {
            let result = match artifact {
                ArtifactRef::Bundle { id } => verify_bundle_file(&path, id, mode),
                ArtifactRef::LooseChunk { hash } => {
                    let entry = chunk_index.entries.get(hash).ok_or_else(|| {
                        Error::repository(format!(
                            "missing chunk index entry for required chunk '{hash}'"
                        ))
                    });
                    entry.and_then(|entry| {
                        read_loose_chunk(&path, *hash, u64::from(entry.raw_size), None).map(|_| ())
                    })
                }
            };
            let result = result.map_err(|error| error.to_string());
            verified.insert(artifact.clone(), result.clone());
            result
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

    Ok(SnapshotArtifactVerifyResult {
        snapshot_id: snapshot_manifest.snapshot_id.clone(),
        ok,
        check_passed,
        required_bundle_count,
        required_artifact_count: required_artifacts.len() + unindexed_chunks.len(),
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

fn verify_bundle_file(path: &Path, bundle_id: &str, mode: RepoArtifactVerification) -> Result<()> {
    let mut reader = bundle::BundleReader::open(File::open(path)?)?;
    if mode == RepoArtifactVerification::Strict {
        reader.verify_identity(bundle_id)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "verify_tests.rs"]
mod tests;

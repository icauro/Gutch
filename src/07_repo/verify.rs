//! Repository artifact verification by structure or identity.

use serde::Deserialize;
use serde::Serialize;
use std::path::Path;
use std::path::PathBuf;

use crate::Error;
use crate::Result;
use crate::artifact::bundle;
use crate::artifact::reference::ArtifactRef;
use crate::artifact::seal::{SealKeys, SealPurpose, StoredReader};
use crate::manifest::SnapshotManifest;
use crate::progress::OperationControl;

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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepoVerifyMode {
    #[default]
    Fast,
    Strict,
}
impl RepoVerifyMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fast => "fast",
            Self::Strict => "strict",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoVerifyResult {
    pub ok: bool,
    pub mode: RepoVerifyMode,
    pub snapshot_count: usize,
    pub snapshots_ok: usize,
    pub snapshots: Vec<SnapshotVerifyOutcome>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotVerifyOutcome {
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

/// A repository, optionally narrowed to one snapshot, and how deeply to check its artifacts.
#[derive(Debug, Clone, Default)]
pub struct RepoVerifyRequest {
    pub repo_root: PathBuf,
    pub snapshot_id: Option<String>,
    pub mode: RepoVerifyMode,
    pub control: OperationControl,
}

pub fn verify_repo(request: &RepoVerifyRequest) -> Result<RepoVerifyResult> {
    let (repo_root, snapshot_id, mode, control) = (
        request.repo_root.as_path(),
        request.snapshot_id.as_deref(),
        request.mode,
        &request.control,
    );
    coordination::read_with_control(repo_root, control, || {
        let (repo_paths, repo_manifest) = load_repo_manifest(repo_root)?;
        let chunk_index = load_chunk_index(&repo_paths)?;
        let constraints = repo_manifest.constraints.clone().unwrap_or_default();

        let mut snapshots = Vec::new();
        let mut verified = HashMap::new();
        for stored_snapshot_id in list_repo_snapshot_ids(&repo_paths)? {
            control.checkpoint()?;
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
            return Err(Error::repo(format!("snapshot not found in repo: {id}")));
        }
        let snapshots_ok = snapshots.iter().filter(|snapshot| snapshot.ok).count();
        let ok = snapshots_ok == snapshots.len();

        Ok(RepoVerifyResult {
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
    mode: RepoVerifyMode,
    verified: &mut VerifiedArtifacts,
) -> Result<SnapshotVerifyOutcome> {
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
                ArtifactRef::Bundle { id } => {
                    verify_bundle_file(&path, id, mode, repo_paths.keys())
                }
                ArtifactRef::LooseChunk { hash } => {
                    let entry = chunk_index.entries.get(hash).ok_or_else(|| {
                        Error::repo(format!(
                            "missing chunk index entry for required chunk '{hash}'"
                        ))
                    });
                    entry.and_then(|entry| {
                        read_loose_chunk(
                            &path,
                            *hash,
                            u64::from(entry.raw_size),
                            repo_paths.keys(),
                            None,
                        )
                        .map(|_| ())
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

    Ok(SnapshotVerifyOutcome {
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

fn verify_bundle_file(
    path: &Path,
    bundle_id: &str,
    mode: RepoVerifyMode,
    keys: Option<&SealKeys>,
) -> Result<()> {
    let file = StoredReader::new(File::open(path)?, keys, SealPurpose::Bundle)?;
    let mut reader = bundle::BundleReader::open(file)?;
    if mode == RepoVerifyMode::Strict {
        reader.verify_identity(bundle_id, keys)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "verify_tests.rs"]
mod tests;

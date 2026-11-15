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
use crate::artifact::profiles::CompressionProfile;
use crate::filesystem::constraints::validate_paths;
use crate::repo::artifacts::required_artifacts;
use crate::repo::coordination;
use crate::repo::index::ChunkIndex;
use crate::repo::index::load_chunk_index;
use crate::repo::paths::RepoPaths;
use crate::repo::paths::load_gutch_repo_manifest;
use crate::repo::snapshot_manifests::load_snapshot_manifest;
use std::collections::HashMap;
use std::fs::File;

/// Structural artifact checks optionally include full content-addressed identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepoArtifactVerification {
    Structure,
    Identity,
}
impl RepoArtifactVerification {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Structure => "fast",
            Self::Identity => "strict",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoArtifactVerifyResult {
    pub ok: bool,
    pub mode: String,
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
pub fn verify_gutch_repo_artifacts(
    repo_root: &Path,
    snapshot_id: Option<&str>,
    mode: RepoArtifactVerification,
) -> Result<RepoArtifactVerifyResult> {
    let operation_root = (repo_root).to_path_buf();
    coordination::read(&operation_root, || {
        let (repo_paths, repo_manifest) = load_gutch_repo_manifest(repo_root)?;
        let chunk_index = load_chunk_index(&repo_paths)?;
        let constraints = repo_manifest.constraints.clone().unwrap_or_default();
        let compression_profile = repo_manifest.compression_profile();

        let mut snapshots = Vec::new();
        let mut verified = HashMap::new();
        for stored_snapshot_id in list_repo_snapshot_ids(&repo_paths)? {
            let manifest = load_snapshot_manifest(&repo_paths, &stored_snapshot_id)?;
            if snapshot_id.is_some_and(|id| id != stored_snapshot_id) {
                // Snapshot selection narrows payload checks, not repository metadata validation.
                required_artifacts(&manifest, &chunk_index)?;
                continue;
            }
            validate_paths(
                &constraints.targets,
                manifest.entries.iter().map(|entry| entry.path()),
                "verify",
                Some(&stored_snapshot_id),
            )?;
            snapshots.push(verify_snapshot_artifacts_cached(
                &repo_paths,
                &chunk_index,
                &compression_profile,
                &manifest,
                mode,
                &mut verified,
            )?);
        }
        if let Some(id) = snapshot_id
            && snapshots.is_empty()
        {
            return Err(Error::manifest(format!("snapshot not found in repo: {id}")));
        }
        let snapshots_ok = snapshots.iter().filter(|snapshot| snapshot.ok).count();
        let ok = snapshots_ok == snapshots.len();

        Ok(RepoArtifactVerifyResult {
            ok,
            mode: mode.as_str().to_string(),
            snapshot_count: snapshots.len(),
            snapshots_ok,
            snapshots,
        })
    })
}

pub(crate) fn verify_snapshot_artifacts(
    repo_paths: &RepoPaths,
    chunk_index: &ChunkIndex,
    compression_profile: &CompressionProfile,
    snapshot_id: &str,
    mode: RepoArtifactVerification,
) -> Result<SnapshotArtifactVerifyResult> {
    let manifest = load_snapshot_manifest(repo_paths, snapshot_id)?;
    verify_snapshot_artifacts_cached(
        repo_paths,
        chunk_index,
        compression_profile,
        &manifest,
        mode,
        &mut HashMap::new(),
    )
}

fn verify_snapshot_artifacts_cached(
    repo_paths: &RepoPaths,
    chunk_index: &ChunkIndex,
    compression_profile: &CompressionProfile,
    snapshot_manifest: &SnapshotManifest,
    mode: RepoArtifactVerification,
    verified: &mut HashMap<ArtifactRef, std::result::Result<(), String>>,
) -> Result<SnapshotArtifactVerifyResult> {
    let required_artifacts = required_artifacts(snapshot_manifest, chunk_index)?;
    let required_bundle_count = required_artifacts
        .iter()
        .filter(|artifact| matches!(artifact, ArtifactRef::Bundle { .. }))
        .count();

    let mut missing_bundles = Vec::new();
    let mut missing_artifacts = Vec::new();
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
                    verify_bundle_file(&path, id, compression_profile, mode)
                }
                ArtifactRef::LooseChunk { hash } => {
                    let entry = chunk_index.entries.get(hash).ok_or_else(|| {
                        Error::manifest(format!(
                            "missing chunk index entry for required chunk '{hash}'"
                        ))
                    });
                    entry.and_then(|entry| {
                        read_loose_chunk(
                            &path,
                            hash,
                            u64::from(entry.raw_size),
                            compression_profile,
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

    Ok(SnapshotArtifactVerifyResult {
        snapshot_id: snapshot_manifest.snapshot_id.clone(),
        ok,
        check_passed,
        required_bundle_count,
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
    compression_profile: &CompressionProfile,
    mode: RepoArtifactVerification,
) -> Result<()> {
    let mut reader = bundle::BundleReader::open(File::open(path)?, compression_profile)?;
    if mode == RepoArtifactVerification::Identity {
        reader.verify_identity(bundle_id)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn save_test_snapshot(paths: &RepoPaths, path: &str, bytes: &[u8]) -> SnapshotManifest {
        let hash = crate::artifact::hash::ChunkHash::of(bytes);
        let mut manifest = SnapshotManifest::new(String::new(), Default::default());
        manifest.push_file(crate::manifest::FileManifest {
            path: path.into(),
            size: bytes.len() as u64,
            executable: false,
            read_only: false,
            full_file_hash: hash.to_hex(),
            chunks: if bytes.is_empty() {
                Vec::new()
            } else {
                vec![crate::manifest::ChunkRef {
                    chunk_hash: hash,
                    raw_size: bytes.len() as u32,
                }]
            },
        });
        manifest.snapshot_id =
            crate::snapshot::identity::compute_snapshot_id_from_entries(&manifest.entries);
        crate::repo::snapshot_manifests::write_snapshot_manifest_artifacts(paths, &manifest)
            .unwrap();
        manifest
    }

    #[test]
    fn selection_limits_payload_checks_but_validates_unselected_chunk_references() {
        let root = tempfile::tempdir().unwrap();
        let paths = RepoPaths::from_repo_root(root.path());
        crate::repo::paths::ensure_gutch_repo_layout(&paths, "selection").unwrap();
        let selected = save_test_snapshot(&paths, "empty", b"");
        let unselected = save_test_snapshot(&paths, "missing", b"unavailable payload");
        crate::test_util::index_manifest(&paths, &unselected);

        for mode in [
            RepoArtifactVerification::Structure,
            RepoArtifactVerification::Identity,
        ] {
            let result =
                verify_gutch_repo_artifacts(root.path(), Some(&selected.snapshot_id), mode)
                    .unwrap();
            assert!(result.ok);
            assert_eq!(result.snapshot_count, 1);
            assert_eq!(result.snapshots[0].snapshot_id, selected.snapshot_id);
            assert_eq!(result.snapshots[0].required_artifact_count, 0);
            let all = verify_gutch_repo_artifacts(root.path(), None, mode).unwrap();
            assert!(!all.ok);
            assert_eq!(all.snapshot_count, 2);
            assert_eq!(all.snapshots_ok, 1);
            let missing = all
                .snapshots
                .iter()
                .find(|item| item.snapshot_id == unselected.snapshot_id)
                .unwrap();
            assert_eq!(missing.missing_bundle_count, 1);
            assert_eq!(missing.missing_artifact_count, 1);
        }

        let mut index = load_chunk_index(&paths).unwrap();
        let chunk = unselected.files().next().unwrap().chunks[0].chunk_hash;
        let mut entry = index.entries.get_hash(&chunk).unwrap().clone();
        entry.raw_size += 1;
        index.entries.insert(chunk, entry);
        crate::repo::index::write_chunk_index(&paths, &index).unwrap();
        let error = verify_gutch_repo_artifacts(
            root.path(),
            Some(&selected.snapshot_id),
            RepoArtifactVerification::Structure,
        )
        .unwrap_err();
        assert!(error.to_string().contains("chunk index size disagrees"));

        crate::repo::index::write_chunk_index(&paths, &crate::repo::index::empty_chunk_index())
            .unwrap();
        let error = verify_gutch_repo_artifacts(
            root.path(),
            Some(&selected.snapshot_id),
            RepoArtifactVerification::Structure,
        )
        .unwrap_err();
        assert!(error.to_string().contains("missing chunk index entry"));

        crate::test_util::index_manifest(&paths, &unselected);
        let mut invalid = unselected;
        let crate::manifest::SnapshotEntry::File(file) = &mut invalid.entries[0] else {
            panic!("expected the fixture's file entry");
        };
        file.full_file_hash = blake3::hash(b"different content").to_hex().to_string();
        crate::repo::snapshot_manifests::write_snapshot_manifest_artifacts(&paths, &invalid)
            .unwrap();
        let error = verify_gutch_repo_artifacts(
            root.path(),
            Some(&selected.snapshot_id),
            RepoArtifactVerification::Structure,
        )
        .unwrap_err();
        assert!(matches!(error, Error::SnapshotIntegrityMismatch { .. }));
    }

    #[test]
    fn empty_repository_succeeds_but_unknown_snapshot_is_an_error() {
        let root = tempfile::tempdir().unwrap();
        let paths = RepoPaths::from_repo_root(root.path());
        crate::repo::paths::ensure_gutch_repo_layout(&paths, "empty").unwrap();
        let result =
            verify_gutch_repo_artifacts(root.path(), None, RepoArtifactVerification::Structure)
                .unwrap();
        assert!(result.ok);
        assert_eq!(result.snapshot_count, 0);
        let unknown = "a".repeat(64);
        let error = verify_gutch_repo_artifacts(
            root.path(),
            Some(&unknown),
            RepoArtifactVerification::Structure,
        )
        .unwrap_err();
        assert!(error.to_string().contains("snapshot not found in repo"));
    }

    #[test]
    fn structural_verification_does_not_claim_content_addressed_identity() {
        let profile = crate::artifact::profiles::CompressionProfile::default();
        let mut builder = bundle::BundleBuilder::new(profile.clone());
        let bytes = b"verified bundle payload";
        builder.add_chunk(crate::artifact::hash::ChunkHash::of(bytes), bytes.to_vec());
        let (bundle_id, encoded) = builder.build_with_id().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("bundle");
        std::fs::write(&path, encoded).unwrap();
        verify_bundle_file(
            &path,
            &bundle_id,
            &profile,
            RepoArtifactVerification::Identity,
        )
        .unwrap();
        let incorrect_id = "0".repeat(64);
        verify_bundle_file(
            &path,
            &incorrect_id,
            &profile,
            RepoArtifactVerification::Structure,
        )
        .unwrap();
        assert!(
            verify_bundle_file(
                &path,
                &incorrect_id,
                &profile,
                RepoArtifactVerification::Identity
            )
            .is_err()
        );
    }
}

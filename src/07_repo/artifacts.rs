//! Required artifacts for snapshots and bundle identity checks.

use crate::artifact::bundle::BundleTableEntry;
use crate::artifact::bundle::{BundleReader, verify_bundle_identity_reader};
use crate::artifact::hash::ChunkHash;
use crate::artifact::profiles::ArtifactStorageMode;
use crate::artifact::reference::ArtifactRef;
use crate::artifact::seal::SealKeys;
use crate::manifest::snapshot::SnapshotManifest;
use crate::repo::index::ChunkIndex;
use crate::repo::index::ChunkIndexEntry;
use crate::repo::paths::RepoPaths;
use crate::repo::paths::bundle_id_from_bundle_path;
use crate::repo::paths::bundle_path;
use crate::repo::paths::chunk_path;
use crate::{Error, Result};
use std::collections::HashSet;
use std::fs::File;
use std::path::{Path, PathBuf};

// Repository lookups for artifact references; the type itself is repository-independent.
impl ArtifactRef {
    pub(crate) fn from_chunk_entry(chunk_hash: &ChunkHash, entry: &ChunkIndexEntry) -> Self {
        match entry.storage_mode {
            ArtifactStorageMode::Bundled => Self::Bundle {
                id: entry.bundle_id.clone(),
            },
            ArtifactStorageMode::LooseChunks => Self::LooseChunk { hash: *chunk_hash },
        }
    }

    pub(crate) fn local_path(&self, repo_paths: &RepoPaths) -> Result<PathBuf> {
        match self {
            Self::Bundle { id } => bundle_path(repo_paths, id),
            Self::LooseChunk { hash } => Ok(chunk_path(repo_paths, *hash)),
        }
    }
}

pub(crate) fn required_artifacts(
    snapshot_manifest: &SnapshotManifest,
    chunk_index: &ChunkIndex,
) -> Result<HashSet<ArtifactRef>> {
    let required = classify_required_artifacts(snapshot_manifest, chunk_index)?;
    if let Some(chunk_hash) = required.unindexed_chunks.first() {
        return Err(Error::repo(format!(
            "missing chunk index entry for required chunk '{chunk_hash}'"
        )));
    }
    Ok(required.artifacts)
}

/// Artifacts a snapshot needs, with the chunks the index cannot place kept apart.
pub(crate) struct RequiredArtifacts {
    pub artifacts: HashSet<ArtifactRef>,
    /// Sorted, distinct chunk hashes that have no chunk index entry.
    pub unindexed_chunks: Vec<String>,
}

/// Like [`required_artifacts`], but an unindexed chunk is reported as missing
/// content instead of aborting, so status and verify can still describe the
/// repository. A size disagreement is corrupt metadata and still fails.
pub(crate) fn classify_required_artifacts(
    snapshot_manifest: &SnapshotManifest,
    chunk_index: &ChunkIndex,
) -> Result<RequiredArtifacts> {
    let mut artifacts = HashSet::new();
    let mut unindexed_chunks = std::collections::BTreeSet::new();
    for chunk_ref in snapshot_manifest
        .files()
        .flat_map(|file| file.chunks.iter())
    {
        let Some(entry) = chunk_index.entries.get(&chunk_ref.chunk_hash) else {
            unindexed_chunks.insert(chunk_ref.chunk_hash.to_string());
            continue;
        };
        if entry.raw_size != chunk_ref.raw_size {
            return Err(Error::repo(format!(
                "chunk index size disagrees with snapshot for '{}'",
                chunk_ref.chunk_hash
            )));
        }
        artifacts.insert(ArtifactRef::from_chunk_entry(&chunk_ref.chunk_hash, entry));
    }
    Ok(RequiredArtifacts {
        artifacts,
        unindexed_chunks: unindexed_chunks.into_iter().collect(),
    })
}

/// Checks a stored bundle against its ID. `keys` are the repository's when it
/// is encrypted.
pub(crate) fn verify_bundle_identity_file(
    path: &Path,
    bundle_id: &str,
    keys: Option<&SealKeys>,
) -> Result<()> {
    let expected_hash = ChunkHash::parse_hex(bundle_id)?;
    let path_id = bundle_id_from_bundle_path(path).ok_or_else(|| {
        crate::error::Error::Artifact(format!(
            "bundle path '{}' does not name a bundle ID",
            path.display()
        ))
    })?;
    if path_id != expected_hash.to_hex() {
        return Err(crate::error::Error::HashMismatch {
            expected: expected_hash.to_hex(),
            actual: path_id,
        });
    }
    verify_bundle_identity_reader(&mut File::open(path)?, bundle_id, keys)
}

/// Read a bundle's table of contents without rendering output.
pub fn inspect_bundle(bundle: &Path) -> Result<Vec<BundleTableEntry>> {
    let reader = BundleReader::open(File::open(bundle)?)?;
    Ok(reader.entries().to_vec())
}

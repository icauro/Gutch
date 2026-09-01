use std::path::PathBuf;

use crate::manifest::{ArtifactStorageMode, ChunkIndexEntry, CompressionProfile};
use crate::{Error, GutchRepoPaths, Result};

/// A persisted artifact that can satisfy one or more logical chunks.
///
/// Bundles and loose chunks differ only at this boundary. Snapshot planning,
/// transfer scheduling, reconstruction, and verification operate on this
/// type instead of branching on the storage mode.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum ArtifactRef {
    Bundle { id: String },
    LooseChunk { hash: String },
}

impl ArtifactRef {
    pub(crate) fn from_chunk_entry(chunk_hash: &str, entry: &ChunkIndexEntry) -> Self {
        match entry.storage_mode {
            ArtifactStorageMode::Bundled => Self::Bundle {
                id: entry.bundle_id.clone(),
            },
            ArtifactStorageMode::LooseChunks => Self::LooseChunk {
                hash: chunk_hash.to_string(),
            },
        }
    }

    pub(crate) fn id(&self) -> &str {
        match self {
            Self::Bundle { id } => id,
            Self::LooseChunk { hash } => hash,
        }
    }

    pub(crate) fn local_path(&self, repo_paths: &GutchRepoPaths) -> Result<PathBuf> {
        match self {
            Self::Bundle { id } => crate::bundle_path(repo_paths, id),
            Self::LooseChunk { hash } => crate::chunk_path(repo_paths, hash),
        }
    }
}

pub(crate) fn required_artifacts(
    snapshot_manifest: &crate::manifest::snapshot::SnapshotManifest,
    chunk_index: &crate::manifest::ChunkIndex,
) -> Result<std::collections::HashSet<ArtifactRef>> {
    snapshot_manifest
        .files()
        .flat_map(|file| file.chunks.iter())
        .map(|chunk_ref| {
            let entry = chunk_index
                .entries
                .get(&chunk_ref.chunk_hash)
                .ok_or_else(|| {
                    Error::manifest(format!(
                        "missing chunk index entry for required chunk '{}'",
                        chunk_ref.chunk_hash
                    ))
                })?;
            Ok(ArtifactRef::from_chunk_entry(&chunk_ref.chunk_hash, entry))
        })
        .collect()
}

pub(crate) fn required_artifacts_from_index(
    snapshot_manifest: &crate::manifest::snapshot::SnapshotManifest,
    chunk_index: &crate::manifest::ChunkIndex,
) -> std::collections::HashSet<ArtifactRef> {
    snapshot_manifest
        .files()
        .flat_map(|file| file.chunks.iter())
        .filter_map(|chunk_ref| {
            chunk_index
                .entries
                .get(&chunk_ref.chunk_hash)
                .map(|entry| ArtifactRef::from_chunk_entry(&chunk_ref.chunk_hash, entry))
        })
        .collect()
}

pub(crate) fn read_loose_chunk(
    path: &std::path::Path,
    chunk_hash: &str,
    raw_size: u64,
    compression_profile: &CompressionProfile,
) -> Result<Vec<u8>> {
    let compressed = std::fs::read(path)?;
    let bytes = crate::compress::decompress_data(&compressed, compression_profile.compression)?;
    if bytes.len() as u64 != raw_size {
        return Err(Error::reconstruct(format!(
            "loose chunk '{}' has size {}, expected {}",
            chunk_hash,
            bytes.len(),
            raw_size
        )));
    }
    let actual_hash = crate::hash::hash_hex(&crate::hash::hash_chunk(&bytes));
    if actual_hash != chunk_hash {
        return Err(Error::HashMismatch {
            expected: chunk_hash.to_string(),
            actual: actual_hash,
        });
    }
    Ok(bytes)
}

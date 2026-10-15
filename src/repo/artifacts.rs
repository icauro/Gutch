use crate::artifact::bundle::BundleTableEntry;
use crate::artifact::bundle::{BundleReader, verify_bundle_identity_reader};
use crate::artifact::hash::ChunkHash;
use crate::artifact::profiles::ArtifactStorageMode;
use crate::repo::index::ChunkIndexEntry;
use crate::repo::paths::GutchRepoPaths;
use crate::{Error, Result};
use std::fs::File;
use std::path::{Path, PathBuf};

/// A persisted artifact that can satisfy one or more logical chunks.
///
/// Bundles and loose chunks differ only at this boundary. Snapshot planning,
/// reconstruction, and verification operate on this type; the remote transfer
/// layer resolves bundle URLs at its provider-specific boundary.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum ArtifactRef {
    // Many chunks share a bundle, so retain the index's shared identifier storage.
    Bundle { id: smol_str::SmolStr },
    LooseChunk { hash: String },
}

impl ArtifactRef {
    pub(crate) fn from_chunk_entry(chunk_hash: &ChunkHash, entry: &ChunkIndexEntry) -> Self {
        match entry.storage_mode {
            ArtifactStorageMode::Bundled => Self::Bundle {
                id: entry.bundle_id.clone(),
            },
            ArtifactStorageMode::LooseChunks => Self::LooseChunk {
                hash: chunk_hash.to_hex(),
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
            Self::Bundle { id } => crate::repo::paths::bundle_path(repo_paths, id),
            Self::LooseChunk { hash } => crate::repo::paths::chunk_path(repo_paths, hash),
        }
    }
}

pub(crate) fn required_artifacts(
    snapshot_manifest: &crate::manifest::snapshot::SnapshotManifest,
    chunk_index: &crate::repo::index::ChunkIndex,
) -> Result<std::collections::HashSet<ArtifactRef>> {
    snapshot_manifest
        .files()
        .flat_map(|file| file.chunks.iter())
        .map(|chunk_ref| {
            let entry = chunk_index
                .entries
                .get_hash(&chunk_ref.chunk_hash)
                .ok_or_else(|| {
                    Error::manifest(format!(
                        "missing chunk index entry for required chunk '{}'",
                        chunk_ref.chunk_hash
                    ))
                })?;
            if entry.raw_size != chunk_ref.raw_size {
                return Err(Error::manifest(format!(
                    "chunk index size disagrees with snapshot for '{}'",
                    chunk_ref.chunk_hash
                )));
            }
            Ok(ArtifactRef::from_chunk_entry(&chunk_ref.chunk_hash, entry))
        })
        .collect()
}

pub(crate) fn verify_bundle_identity_file(path: &Path, bundle_id: &str) -> Result<()> {
    let expected_hash = ChunkHash::parse_hex(bundle_id)?;
    let path_id = crate::repo::paths::bundle_id_from_bundle_path(path).ok_or_else(|| {
        crate::error::Error::InvalidBundle(format!(
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
    verify_bundle_identity_reader(&mut File::open(path)?, bundle_id)
}

/// Inspect a bundle using its repository's compression profile without rendering output.
pub fn inspect_bundle(bundle: &Path, repo: &Path) -> Result<Vec<BundleTableEntry>> {
    let (_, manifest) = crate::repo::paths::load_gutch_repo_manifest(repo)?;
    let reader = BundleReader::open(File::open(bundle)?, &manifest.compression_profile())?;
    Ok(reader.entries().to_vec())
}

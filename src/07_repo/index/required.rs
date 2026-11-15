//! Chunks and bundles a snapshot needs.

use super::records::ChunkIndex;
use crate::Result;
use crate::artifact::hash::ChunkHash;
use crate::artifact::reference::ArtifactRef;
use crate::manifest::snapshot::SnapshotManifest;
use crate::repo::artifacts::required_artifacts;
use std::collections::HashSet;

pub fn required_chunk_hashes(snapshot_manifest: &SnapshotManifest) -> HashSet<ChunkHash> {
    snapshot_manifest
        .files()
        .flat_map(|file| file.chunks.iter().map(|chunk| chunk.chunk_hash))
        .collect()
}

pub fn required_bundle_ids(
    snapshot_manifest: &SnapshotManifest,
    chunk_index: &ChunkIndex,
) -> Result<HashSet<String>> {
    Ok(required_artifacts(snapshot_manifest, chunk_index)?
        .into_iter()
        .filter_map(|artifact| match artifact {
            ArtifactRef::Bundle { id } => Some(id.to_string()),
            ArtifactRef::LooseChunk { .. } => None,
        })
        .collect())
}

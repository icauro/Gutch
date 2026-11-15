//! Chunk index records: indexed artifacts and their chunk entries.

use crate::artifact::profiles::ArtifactStorageMode;
use serde::{Deserialize, Serialize};
use std::ops::Deref;
use std::sync::Arc;
/// The binary chunk map carries its own format header, so the record holds
/// only entries.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct ChunkIndex {
    pub entries: super::entries::ChunkIndexEntries,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IndexedArtifact {
    pub artifact_size: u32,
    pub bundle_id: smol_str::SmolStr,
    pub storage_mode: ArtifactStorageMode,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChunkIndexEntry {
    #[serde(flatten)]
    pub artifact: Arc<IndexedArtifact>,
    pub chunk_ordinal: u32,
    pub raw_size: u32,
}
impl Deref for ChunkIndexEntry {
    type Target = IndexedArtifact;
    fn deref(&self) -> &Self::Target {
        &self.artifact
    }
}
impl ChunkIndexEntry {
    pub fn new(
        artifact_size: u32,
        chunk_ordinal: u32,
        bundle_id: smol_str::SmolStr,
        raw_size: u32,
        storage_mode: ArtifactStorageMode,
    ) -> Self {
        Self {
            artifact: Arc::new(IndexedArtifact {
                artifact_size,
                bundle_id,
                storage_mode,
            }),
            chunk_ordinal,
            raw_size,
        }
    }
}

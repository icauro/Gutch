use crate::artifact::profiles::ArtifactStorageMode;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkIndex {
    pub format_version: String,
    pub entries: super::entries::ChunkIndexEntries,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IndexedArtifact {
    pub artifact_size: u32,
    pub bundle_id: smol_str::SmolStr,
    pub storage_mode: ArtifactStorageMode,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ChunkIndexEntry {
    #[serde(flatten)]
    pub artifact: Arc<IndexedArtifact>,
    pub chunk_ordinal: u32,
    pub raw_size: u32,
}
impl std::ops::Deref for ChunkIndexEntry {
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

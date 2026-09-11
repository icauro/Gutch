use crate::manifest::ArtifactStorageMode;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkIndex {
    pub format_version: String,
    pub updated_at: String,
    pub entries: super::entries::ChunkIndexEntries,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ChunkIndexEntry {
    pub artifact_size: u32,
    pub chunk_ordinal: u32,
    pub bundle_id: smol_str::SmolStr,
    pub raw_size: u32,
    pub compressed_size: u32,
    #[serde(default)]
    pub storage_mode: ArtifactStorageMode,
}

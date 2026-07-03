use serde::{Deserialize, Serialize};

use super::ArtifactStorageMode;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileManifest {
    pub path: String,
    pub size: u64,
    pub executable: bool,
    pub full_file_hash: String,
    pub chunks: Vec<ChunkRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkRef {
    pub chunk_hash: String,
    pub raw_size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleTableEntry {
    pub chunk_hash: String,
    pub raw_size: u64,
    pub compressed_size: u64,
    pub compressed_offset: u64,
    #[serde(default)]
    pub raw_offset: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Estimate {
    pub missing_logical_bytes: u64,
    pub missing_compressed_bytes: u64,
    pub missing_chunk_count: u64,
    pub required_bundle_count: u64,
    pub range_request_count: u64,
    pub local_cache_hits: u64,
    pub available_disk_space: u64,
    pub fragmentation_score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkIndex {
    pub format_version: String,
    pub updated_at: String,
    pub entries: std::collections::BTreeMap<String, ChunkIndexEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ChunkIndexEntry {
    pub bundle_id: String,
    pub raw_size: u64,
    pub compressed_size: u64,
    #[serde(default)]
    pub storage_mode: ArtifactStorageMode,
}

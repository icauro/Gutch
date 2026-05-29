use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::manifest::{
    BundleSizingProfile, ChunkSizingProfile, CompressionProfile, FileManifest, TargetTagSet,
    target::TargetManifest,
};
use crate::runtime::{RuntimePlan, RuntimeResourceHints};

#[derive(Debug, Clone)]
pub struct BuildConfig {
    pub content_repo_id: String,
    pub target_id: String,
    pub target_tags: TargetTagSet,
    pub existing_chunk_locations: HashMap<String, ExistingChunkLocation>,
    pub existing_bundle_ids: HashSet<String>,
    pub previous_file_manifests: HashMap<String, FileManifest>,
    pub chunk_profile: ChunkSizingProfile,
    pub bundle_profile: BundleSizingProfile,
    pub compression_profile: CompressionProfile,
    pub runtime_hints: RuntimeResourceHints,
}

impl BuildConfig {
    pub fn with_ids(
        content_repo_id: &str,
        target_id: &str,
        target_tags: &TargetTagSet,
    ) -> Self {
        Self {
            content_repo_id: content_repo_id.to_string(),
            target_id: target_id.to_string(),
            target_tags: target_tags.clone(),
            existing_chunk_locations: HashMap::new(),
            existing_bundle_ids: HashSet::new(),
            previous_file_manifests: HashMap::new(),
            chunk_profile: ChunkSizingProfile::default(),
            bundle_profile: BundleSizingProfile::default(),
            compression_profile: CompressionProfile::default(),
            runtime_hints: RuntimeResourceHints::default(),
        }
    }
}

pub struct BuildResult {
    pub target_manifest: TargetManifest,
    pub bundles: Vec<BuiltBundle>,
}

#[derive(Debug, Clone)]
pub struct BuiltBundle {
    pub bundle_id: String,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct ExistingChunkLocation {
    pub bundle_id: String,
    pub raw_size: u64,
    pub compressed_size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "storage")]
pub enum ChunkPayload {
    InMemory {
        bytes: Vec<u8>,
    },
    TempFile {
        path: PathBuf,
        raw_size: u64,
    },
}

impl ChunkPayload {
    pub fn raw_size(&self) -> u64 {
        match self {
            ChunkPayload::InMemory { bytes } => bytes.len() as u64,
            ChunkPayload::TempFile { raw_size, .. } => *raw_size,
        }
    }

    pub fn load_bytes(&self) -> crate::Result<Vec<u8>> {
        match self {
            ChunkPayload::InMemory { bytes } => Ok(bytes.clone()),
            ChunkPayload::TempFile { path, .. } => Ok(std::fs::read(path)?),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct BuildTimings {
    pub scan_read: Duration,
    pub scan_read_bytes: u64,
    pub file_hash: Duration,
    pub file_hash_bytes: u64,
    pub chunking: Duration,
    pub chunking_bytes: u64,
    pub chunk_hash_judgment: Duration,
    pub chunk_hash_judgment_bytes: u64,
    pub metadata: Duration,
    pub dedupe_lookup: Duration,
    pub dedupe_lookup_bytes: u64,
    pub compression: Duration,
    pub compression_input_bytes: u64,
    pub compression_output_bytes: u64,
    pub total_compressed_bytes: u64,
    pub bundle_compressed_payload_load: Duration,
    pub bundle_compressed_payload_load_bytes: u64,
    pub bundle_compressed_payload_assembly: Duration,
    pub bundle_compressed_payload_assembly_bytes: u64,
    pub bundle_compressed_stream_compression: Duration,
    pub bundle_compressed_stream_input_bytes: u64,
    pub bundle_compressed_stream_output_bytes: u64,
    pub bundle_compressed_serialization: Duration,
    pub bundle_compressed_serialization_bytes: u64,
    pub bundle_build: Duration,
    pub bundle_build_bytes: u64,
    pub target_manifest_finalize: Duration,
    pub total: Duration,
    pub runtime_plan: Option<RuntimePlan>,
}

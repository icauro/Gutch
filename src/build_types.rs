use std::path::PathBuf;
use std::time::Duration;

use crate::manifest::{TargetManifest, TargetTagSet};

#[derive(Debug, Clone)]
pub struct RepoInitTargetInput {
    pub dir: PathBuf,
    pub target_id: String,
    pub target_tags: TargetTagSet,
}

#[derive(Debug, Clone)]
pub struct RepoInitResult {
    pub target_manifests: Vec<TargetManifest>,
    pub bundles: Vec<BuiltBundle>,
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
    pub bundle_build: Duration,
    pub bundle_build_bytes: u64,
    pub target_manifest_finalize: Duration,
    pub total: Duration,
}

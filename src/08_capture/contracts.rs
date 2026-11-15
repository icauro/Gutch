//! Capture requests, plans and typed outcomes.
use crate::error::capture_outcome::SnapshotCaptureOutcome;
use crate::manifest::SnapshotTagSet;
use crate::runtime::RuntimeResourceHints;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotCapturePlan {
    pub folder_name: String,
    pub path: String,
    pub version_tag: String,
    pub os: String,
    pub arch: String,
    #[serde(rename = "tags")]
    pub snapshot_tags: SnapshotTagSet,
}

#[derive(Debug, Clone)]
pub struct SnapshotCaptureRequest {
    pub repo_root: PathBuf,
    pub source_dir: PathBuf,
    pub explicit_tags: SnapshotTagSet,
    pub chunk_profile: crate::artifact::profiles::ChunkSizingPreset,
    pub chunker: crate::artifact::profiles::Chunker,
    pub bundle_profile: crate::artifact::profiles::BundleSizingPreset,
    pub compression_algorithm: crate::artifact::profiles::CompressionKind,
    pub compression_level: Option<i32>,
    pub storage_mode: crate::artifact::profiles::ArtifactStorageMode,
    pub runtime_hints: RuntimeResourceHints,
    pub control: crate::progress::OperationControl,
}

#[derive(Debug, Clone)]
pub struct SnapshotCaptureResult {
    pub repo_root: PathBuf,
    pub snapshot_id: String,
    pub outcome: SnapshotCaptureOutcome,
    pub manifest_path: PathBuf,
    pub version_tag: String,
    pub os: String,
    pub arch: String,
    pub file_count: usize,
    pub total_logical_size: u64,
    pub total_compressed_size_estimate: u64,
    pub required_chunk_count: usize,
    pub required_bundle_count: usize,
    pub generated_bundle_count: usize,
    pub generated_chunk_count: usize,
    pub reused_chunk_count: usize,
    pub new_chunk_count: usize,
    pub(crate) build_timings: crate::capture::types::BuildTimings,
}

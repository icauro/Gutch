//! Capture requests, plans and typed outcomes.
use crate::artifact::profiles::ArtifactStorageMode;
use crate::artifact::profiles::BundleSizingPreset;
use crate::artifact::profiles::ChunkSizingPreset;
use crate::artifact::profiles::Chunker;
use crate::artifact::profiles::CompressionKind;
use crate::capture::build::types::BuildTimings;
use crate::error::capture_outcome::SnapshotCaptureOutcome;
use crate::manifest::SnapshotTagSet;
use crate::progress::OperationControl;
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
    pub chunk_profile: ChunkSizingPreset,
    pub chunker: Chunker,
    pub bundle_profile: BundleSizingPreset,
    pub compression_algorithm: CompressionKind,
    pub compression_level: Option<i32>,
    pub storage_mode: ArtifactStorageMode,
    pub runtime_hints: RuntimeResourceHints,
    pub control: OperationControl,
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
    pub(crate) build_timings: BuildTimings,
}

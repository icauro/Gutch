//! Capture requests, plans and typed outcomes.
use crate::manifest::SnapshotTagSet;
use crate::runtime::RuntimeResourceHints;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// One differing metadata field between a capture candidate and the stored
/// snapshot with the same content-derived ID.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MetadataFieldDifference {
    pub field: String,
    pub existing: String,
    pub requested: String,
}

/// Typed outcome when capture computes a content-derived snapshot ID that
/// already exists in the repo.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SnapshotCaptureOutcome {
    Created {
        snapshot_id: String,
    },
    Unchanged {
        snapshot_id: String,
    },
    MetadataConflict {
        snapshot_id: String,
        differences: Vec<MetadataFieldDifference>,
    },
    IntegrityConflict {
        snapshot_id: String,
    },
}

impl SnapshotCaptureOutcome {
    /// Caller-facing wording for the outcome. Adapters render this message
    /// without inferring their own tag-merging or overwrite policy.
    pub fn message(&self) -> String {
        match self {
            SnapshotCaptureOutcome::Created { snapshot_id } => {
                format!("Captured snapshot {snapshot_id}.")
            }
            SnapshotCaptureOutcome::Unchanged { .. } => {
                String::from("Snapshot content already exists; no changes were made.")
            }
            SnapshotCaptureOutcome::MetadataConflict {
                snapshot_id,
                differences,
            } => {
                let mut message = format!("snapshot_id: {snapshot_id}");
                for difference in differences {
                    message.push_str(&format!(
                        "\nfield '{}': existing '{}', requested '{}'",
                        difference.field, difference.existing, difference.requested
                    ));
                }
                message.push_str(
                    "\nCapture did not change the existing snapshot. Use an explicit metadata update for this snapshot ID.",
                );
                message
            }
            SnapshotCaptureOutcome::IntegrityConflict { snapshot_id } => {
                format!(
                    "Existing snapshot data does not match its content-derived ID. The repository may be corrupt.\nsnapshot_id: {snapshot_id}"
                )
            }
        }
    }
}

impl std::fmt::Display for SnapshotCaptureOutcome {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message())
    }
}

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
    pub control: crate::progress::GutchOperationControl,
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

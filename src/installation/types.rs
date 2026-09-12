use serde::Deserialize;
use serde::Serialize;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use crate::installation::inspect::RepairClassification;
use crate::installation::inspect::VerifyMode;
use crate::installation::inspect::VerifyTimings;
use crate::manifest::ExternalProductDependencyRequirement;
use crate::manifest::launch::ResolvedLaunchConfiguration;
use crate::manifest::snapshot::SnapshotManifest;
use crate::progress::GutchOperationControl;
use crate::runtime::RuntimePlan;
use crate::runtime::RuntimeResourceHints;

#[derive(Debug, Clone, Default)]
pub struct ReconstructTimings {
    pub(crate) runtime_plan: Option<RuntimePlan>,
    pub(crate) verify_chunks: bool,
    pub(crate) verify_files: bool,
    pub(crate) prepare_wall: Duration,
    pub(crate) reconstruct_wall: Duration,
    pub(crate) finalize_wall: Duration,
    pub(crate) promote_wall: Duration,
    pub(crate) bundle_read: Duration,
    pub(crate) bundle_read_compressed_bytes: u64,
    pub(crate) bundle_read_decompressed_bytes: u64,
    pub(crate) artifact_read: Duration,
    pub(crate) artifact_read_compressed_bytes: u64,
    pub(crate) artifact_read_decompressed_bytes: u64,
    pub(crate) chunk_verify: Duration,
    pub(crate) chunk_verify_bytes: u64,
    pub(crate) file_rebuild: Duration,
    pub(crate) file_rebuild_bytes: u64,
    pub(crate) file_verify: Duration,
    pub(crate) file_verify_bytes: u64,
    pub(crate) file_write: Duration,
    pub(crate) file_write_bytes: u64,
    pub(crate) total: Duration,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedSnapshotOperationState {
    pub snapshot_id: String,
    pub snapshot_metadata_hash_b3: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch: Option<ResolvedLaunchConfiguration>,
    #[serde(default)]
    pub external_product_dependencies: Vec<ExternalProductDependencyRequirement>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct SnapshotReconstructSource<'a> {
    pub snapshot_manifest: &'a SnapshotManifest,
    pub destination_dir: &'a Path,
}

#[derive(Debug, Clone)]
pub struct SnapshotReconstructResult {
    pub snapshot_id: String,
    pub destination_dir: PathBuf,
    pub staging_dir: PathBuf,
    pub reused_source_chunk_count: usize,
    pub required_chunk_count: usize,
    pub required_bundle_count: usize,
    pub required_artifact_count: usize,
    pub(crate) reconstruct_timings: ReconstructTimings,
    pub(crate) verify_timings: VerifyTimings,
}

#[derive(Debug, Clone)]
pub struct InstallationRequest {
    pub repo_path: PathBuf,
    pub snapshot_id: String,
    pub destination_dir: PathBuf,
    pub staging_dir: Option<PathBuf>,
    pub source_snapshot_id: Option<String>,
    pub source_dir: Option<PathBuf>,
    pub runtime_hints: RuntimeResourceHints,
    pub control: GutchOperationControl,
}

#[derive(Debug, Clone)]
pub struct InstallationReconstructResult {
    pub repo_root: PathBuf,
    pub snapshot_manifest: SnapshotManifest,
    pub reconstruct_result: SnapshotReconstructResult,
    pub resolved_bundle_file_count: usize,
    pub resolved_artifact_file_count: usize,
    pub source_destination_dir: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct InstallationVerifyRequest {
    pub repo_path: PathBuf,
    pub snapshot_id: String,
    pub destination_dir: PathBuf,
    pub verify_mode: VerifyMode,
    pub runtime_hints: RuntimeResourceHints,
    pub control: GutchOperationControl,
}

#[derive(Debug, Clone)]
pub struct InstallationVerifyResult {
    pub repo_root: PathBuf,
    pub snapshot_manifest: SnapshotManifest,
    pub classification: RepairClassification,
    pub(crate) verify_timings: VerifyTimings,
}

/// A deterministic set of destructive mutations used by integration tests and benchmark lanes.
///
/// Paths are always interpreted relative to the reconstructed destination root. The command
/// adapter requires explicit confirmation before applying a plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallationCorruptionPlan {
    pub operations: Vec<InstallationCorruptionOperation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum InstallationCorruptionOperation {
    FlipByte {
        path: String,
        offset: u64,
        #[serde(default = "default_corruption_mask")]
        mask: u8,
    },
    Delete {
        path: String,
    },
    AddFile {
        path: String,
        content: Vec<u8>,
    },
    SetReadOnly {
        path: String,
        read_only: bool,
    },
}

fn default_corruption_mask() -> u8 {
    1
}

#[derive(Debug, Clone)]
pub struct InstallationCorruptionRequest {
    pub destination_dir: PathBuf,
    pub plan: InstallationCorruptionPlan,
    pub control: GutchOperationControl,
}

#[derive(Debug, Clone)]
pub struct InstallationCorruptionResult {
    pub destination_dir: PathBuf,
    pub applied_operation_count: usize,
}

#[derive(Debug, Clone)]
pub struct SnapshotRepairResult {
    pub snapshot_id: String,
    pub destination_dir: PathBuf,
    pub missing_file_count: usize,
    pub corrupt_file_count: usize,
    pub extra_file_count: usize,
    pub removed_extra_file_count: usize,
    pub reconstructed_extra_file_count: usize,
    pub reconstruct_result: Option<SnapshotReconstructResult>,
}

#[derive(Debug, Clone)]
pub struct InstallationRepairRequest {
    pub repo_path: PathBuf,
    pub snapshot_id: String,
    pub destination_dir: PathBuf,
    pub staging_dir: Option<PathBuf>,
    pub remove_added_files: bool,
    pub runtime_hints: RuntimeResourceHints,
    pub control: GutchOperationControl,
}

#[derive(Debug, Clone)]
pub struct InstallationRepairPlanRequest {
    pub repo_path: PathBuf,
    pub snapshot_id: String,
    pub destination_dir: PathBuf,
    pub remove_added_files: bool,
    pub runtime_hints: RuntimeResourceHints,
    pub control: GutchOperationControl,
}

#[derive(Debug, Clone)]
pub struct InstallationRepairPlanResult {
    pub repo_root: PathBuf,
    pub snapshot_manifest: SnapshotManifest,
    pub missing_file_count: usize,
    pub corrupt_file_count: usize,
    pub extra_file_count: usize,
    pub needs_rebuild: bool,
    pub required_bundle_ids: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct InstallationRepairResult {
    pub repo_root: PathBuf,
    pub snapshot_manifest: SnapshotManifest,
    pub repair_result: SnapshotRepairResult,
    pub resolved_bundle_file_count: usize,
}

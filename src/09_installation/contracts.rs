//! Installation requests, results and timings shared by the operations.

use serde::Deserialize;
use serde::Serialize;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use crate::installation::inspect::InstallationIssues;
use crate::installation::inspect::InstallationVerifyMode;
use crate::installation::inspect::VerifyTimings;
use crate::manifest::ExternalProductDependencyRequirement;
use crate::manifest::launch::ResolvedLaunchConfiguration;
use crate::manifest::snapshot::SnapshotManifest;
use crate::progress::OperationControl;
use crate::runtime::RuntimePlan;
use crate::runtime::RuntimeResourceHints;

#[derive(Debug, Clone, Default)]
pub struct ReconstructTimings {
    pub(crate) runtime_plan: Option<RuntimePlan>,
    pub(crate) verify_files: bool,
    pub(crate) prepare_wall: Duration,
    pub(crate) reconstruct_wall: Duration,
    pub(crate) finalize_wall: Duration,
    pub(crate) promote_wall: Duration,
    pub(crate) artifact_read: Duration,
    pub(crate) artifact_read_compressed_bytes: u64,
    pub(crate) artifact_read_decompressed_bytes: u64,
    pub(crate) chunk_verify: Duration,
    pub(crate) chunk_verify_bytes: u64,
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

#[derive(Debug, Clone, Default)]
pub struct InstallationReconstructRequest {
    pub repo_root: PathBuf,
    pub snapshot_id: String,
    pub destination_dir: PathBuf,
    pub staging_dir: Option<PathBuf>,
    pub reuse_snapshot_id: Option<String>,
    pub reuse_dir: Option<PathBuf>,
    pub runtime_hints: RuntimeResourceHints,
    pub control: OperationControl,
}

/// What a reconstruction installed and how much existing content it reused.
#[derive(Debug, Clone)]
pub struct InstallationReconstructResult {
    pub repo_root: PathBuf,
    pub snapshot_manifest: SnapshotManifest,
    pub snapshot_id: String,
    pub destination_dir: PathBuf,
    pub staging_dir: PathBuf,
    /// The folder content was reused from, when the request named one.
    pub reuse_dir: Option<PathBuf>,
    /// Chunks copied from content already on disk instead of read from the repository.
    pub reused_chunk_count: usize,
    pub required_chunk_count: usize,
    pub required_bundle_count: usize,
    pub required_artifact_count: usize,
    pub(crate) reconstruct_timings: ReconstructTimings,
    pub(crate) verify_timings: VerifyTimings,
}

#[derive(Debug, Clone, Default)]
pub struct InstallationVerifyRequest {
    pub repo_root: PathBuf,
    pub snapshot_id: String,
    pub destination_dir: PathBuf,
    pub verify_mode: InstallationVerifyMode,
    pub runtime_hints: RuntimeResourceHints,
    pub control: OperationControl,
}

#[derive(Debug, Clone)]
pub struct InstallationVerifyResult {
    pub repo_root: PathBuf,
    pub snapshot_manifest: SnapshotManifest,
    pub issues: InstallationIssues,
    pub(crate) verify_timings: VerifyTimings,
}

/// A deterministic set of destructive mutations used by integration tests and benchmark lanes.
///
/// Paths are always interpreted relative to the reconstructed destination root. The command
/// adapter requires explicit confirmation before applying a plan.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallationCorruptPlan {
    pub operations: Vec<InstallationCorruptOperation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum InstallationCorruptOperation {
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

#[derive(Debug, Clone, Default)]
pub struct InstallationCorruptRequest {
    pub destination_dir: PathBuf,
    pub plan: InstallationCorruptPlan,
    pub control: OperationControl,
}

#[derive(Debug, Clone)]
pub struct InstallationCorruptResult {
    pub destination_dir: PathBuf,
    pub applied_operation_count: usize,
}

/// What a selective publication found and changed in the destination.
#[derive(Debug, Clone)]
pub(crate) struct InstallationRepairOutcome {
    pub missing_file_count: usize,
    pub corrupt_file_count: usize,
    pub extra_file_count: usize,
    pub removed_extra_file_count: usize,
}

#[derive(Debug, Clone, Default)]
pub struct InstallationRepairRequest {
    pub repo_root: PathBuf,
    pub snapshot_id: String,
    pub destination_dir: PathBuf,
    pub staging_dir: Option<PathBuf>,
    pub remove_extra_files: bool,
    pub runtime_hints: RuntimeResourceHints,
    pub control: OperationControl,
}

#[derive(Debug, Clone)]
pub struct InstallationRepairPlan {
    pub repo_root: PathBuf,
    pub snapshot_manifest: SnapshotManifest,
    pub missing_file_count: usize,
    pub corrupt_file_count: usize,
    pub extra_file_count: usize,
    pub needs_rebuild: bool,
    pub required_artifact_ids: Vec<String>,
}

/// What a repair found, replaced and removed.
#[derive(Debug, Clone)]
pub struct InstallationRepairResult {
    pub repo_root: PathBuf,
    pub snapshot_manifest: SnapshotManifest,
    pub snapshot_id: String,
    pub destination_dir: PathBuf,
    pub missing_file_count: usize,
    pub corrupt_file_count: usize,
    pub extra_file_count: usize,
    pub removed_extra_file_count: usize,
    pub required_bundle_count: usize,
}

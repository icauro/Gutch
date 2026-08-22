use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::manifest::ExternalProductDependencyRequirement;
use crate::manifest::launch::ResolvedLaunchConfiguration;
use crate::manifest::snapshot::SnapshotManifest;
use crate::progress::GutchOperationControl;
use crate::runtime::RuntimeResourceHints;
use crate::verify::{RepairClassification, VerifyMode, VerifyTimings};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GutchBundleDownload {
    pub bundle_id: String,
    #[serde(alias = "downloadUrl")]
    pub url: String,
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

#[derive(Debug, Clone)]
pub struct SnapshotReconstructSource<'a> {
    pub snapshot_manifest: &'a SnapshotManifest,
    pub destination_dir: &'a Path,
}

#[derive(Debug, Clone)]
pub struct SnapshotReconstructResult {
    pub snapshot_id: String,
    pub destination_dir: PathBuf,
    pub staging_dir: PathBuf,
    pub backup_dir: Option<PathBuf>,
    pub reused_source_chunk_count: usize,
    pub required_chunk_count: usize,
    pub required_bundle_count: usize,
    pub reconstruct_timings: crate::ReconstructTimings,
    pub verify_timings: VerifyTimings,
}

#[derive(Debug, Clone)]
pub struct SnapshotReconstructFromRepoRequest {
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
pub struct SnapshotReconstructFromRepoResult {
    pub repo_root: PathBuf,
    pub snapshot_manifest: SnapshotManifest,
    pub reconstruct_result: SnapshotReconstructResult,
    pub resolved_bundle_file_count: usize,
    pub source_destination_dir: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct SnapshotVerifyFromRepoRequest {
    pub repo_path: PathBuf,
    pub snapshot_id: String,
    pub destination_dir: PathBuf,
    pub verify_mode: VerifyMode,
    pub runtime_hints: RuntimeResourceHints,
    pub control: GutchOperationControl,
}

#[derive(Debug, Clone)]
pub struct SnapshotVerifyFromRepoResult {
    pub repo_root: PathBuf,
    pub snapshot_manifest: SnapshotManifest,
    pub classification: RepairClassification,
    pub verify_timings: VerifyTimings,
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
    pub verify_timings: VerifyTimings,
}

#[derive(Debug, Clone)]
pub struct SnapshotRepairFromRepoRequest {
    pub repo_path: PathBuf,
    pub snapshot_id: String,
    pub destination_dir: PathBuf,
    pub staging_dir: Option<PathBuf>,
    pub remove_added_files: bool,
    pub runtime_hints: RuntimeResourceHints,
    pub control: GutchOperationControl,
}

#[derive(Debug, Clone)]
pub struct SnapshotRepairPlanFromRepoRequest {
    pub repo_path: PathBuf,
    pub snapshot_id: String,
    pub destination_dir: PathBuf,
    pub remove_added_files: bool,
    pub runtime_hints: RuntimeResourceHints,
    pub control: GutchOperationControl,
}

#[derive(Debug, Clone)]
pub struct SnapshotRepairPlanFromRepoResult {
    pub repo_root: PathBuf,
    pub snapshot_manifest: SnapshotManifest,
    pub missing_file_count: usize,
    pub corrupt_file_count: usize,
    pub extra_file_count: usize,
    pub needs_rebuild: bool,
    pub required_bundle_ids: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SnapshotRepairFromRepoResult {
    pub repo_root: PathBuf,
    pub snapshot_manifest: SnapshotManifest,
    pub repair_result: SnapshotRepairResult,
    pub resolved_bundle_file_count: usize,
}

#[derive(Debug, Clone)]
pub struct SnapshotFetchRequest {
    pub repo_path: PathBuf,
    pub snapshot_id: String,
    pub repo_metadata_url: Option<String>,
    pub metadata_url: String,
    pub product_metadata_url: Option<String>,
    pub blueprint_url: String,
    pub chunk_index_url: Option<String>,
    pub bundle_downloads_endpoint: Option<String>,
    pub session_id: Option<String>,
    pub bundles: Vec<GutchBundleDownload>,
    pub control: GutchOperationControl,
}

#[derive(Debug, Clone)]
pub struct SnapshotFetchResult {
    pub repo_path: PathBuf,
    pub snapshot_id: String,
    pub required_bundle_count: usize,
    pub downloaded_bundle_count: usize,
    pub skipped_existing_bundle_count: usize,
}

#[derive(Debug, Clone)]
pub struct SnapshotDownloadAndReconstructRequest {
    pub fetch_request: SnapshotFetchRequest,
    pub destination_dir: PathBuf,
    pub staging_dir: Option<PathBuf>,
    pub source_snapshot_id: Option<String>,
    pub source_dir: Option<PathBuf>,
    pub runtime_hints: RuntimeResourceHints,
}

#[derive(Debug, Clone)]
pub struct SnapshotDownloadAndReconstructResult {
    pub fetch_result: SnapshotFetchResult,
    pub reconstruct_result: SnapshotReconstructFromRepoResult,
}

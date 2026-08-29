use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::manifest::ExternalProductDependencyRequirement;
use crate::manifest::launch::ResolvedLaunchConfiguration;
use crate::manifest::snapshot::SnapshotManifest;
use crate::progress::GutchOperationControl;
use crate::runtime::{RuntimePlan, RuntimeResourceHints};
use crate::verify::{RepairClassification, VerifyMode, VerifyTimings};

#[derive(Debug, Clone, Default)]
pub struct ReconstructTimings {
    pub(crate) runtime_plan: Option<RuntimePlan>,
    pub(crate) verify_chunks: bool,
    pub(crate) verify_files: bool,
    pub(crate) prepare_wall: Duration,
    pub(crate) reconstruct_wall: Duration,
    pub(crate) bundle_read: Duration,
    pub(crate) bundle_read_compressed_bytes: u64,
    pub(crate) bundle_read_decompressed_bytes: u64,
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
#[serde(rename_all = "camelCase")]
pub struct BundleDownload {
    pub bundle_id: String,
    #[serde(alias = "downloadUrl")]
    pub url: String,
}

pub trait BundleUrlProvider: Send + Sync {
    fn resolve_bundle_urls(
        &self,
        bundle_ids: &[String],
        control: &GutchOperationControl,
    ) -> crate::Result<Vec<BundleDownload>>;
}

impl<F> BundleUrlProvider for F
where
    F: Fn(&[String], &GutchOperationControl) -> crate::Result<Vec<BundleDownload>> + Send + Sync,
{
    fn resolve_bundle_urls(
        &self,
        bundle_ids: &[String],
        control: &GutchOperationControl,
    ) -> crate::Result<Vec<BundleDownload>> {
        self(bundle_ids, control)
    }
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
    pub(crate) reconstruct_timings: ReconstructTimings,
    pub(crate) verify_timings: VerifyTimings,
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
    pub(crate) verify_timings: VerifyTimings,
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

#[derive(Clone)]
pub struct SnapshotFetchRequest {
    pub repo_path: PathBuf,
    pub snapshot_id: String,
    pub repo_metadata_url: Option<String>,
    pub metadata_url: String,
    pub product_metadata_url: Option<String>,
    pub blueprint_url: String,
    pub chunk_index_url: Option<String>,
    pub bundles: Vec<BundleDownload>,
    pub bundle_url_provider: Option<std::sync::Arc<dyn BundleUrlProvider>>,
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

#[derive(Clone)]
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

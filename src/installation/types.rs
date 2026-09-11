use serde::Deserialize;
use serde::Serialize;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use crate::manifest::ExternalProductDependencyRequirement;
use crate::manifest::launch::ResolvedLaunchConfiguration;
use crate::manifest::snapshot::SnapshotManifest;
use crate::progress::GutchOperationControl;
use crate::runtime::RuntimePlan;
use crate::runtime::RuntimeResourceHints;
use crate::verify::RepairClassification;
use crate::verify::VerifyMode;
use crate::verify::VerifyTimings;

pub const DEFAULT_TRANSFER_CONCURRENCY: usize = 4;
pub const DEFAULT_ADAPTIVE_TRANSFER_MAXIMUM: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferConcurrencyPolicy {
    Fixed(usize),
    Adaptive { initial: usize, maximum: usize },
}

impl Default for TransferConcurrencyPolicy {
    fn default() -> Self {
        Self::Fixed(DEFAULT_TRANSFER_CONCURRENCY)
    }
}

impl TransferConcurrencyPolicy {
    pub const fn fixed(concurrency: usize) -> Self {
        Self::Fixed(concurrency)
    }

    pub const fn adaptive(initial: usize, maximum: usize) -> Self {
        Self::Adaptive { initial, maximum }
    }

    pub const fn initial(self) -> usize {
        match self {
            Self::Fixed(concurrency) => concurrency,
            Self::Adaptive { initial, .. } => initial,
        }
    }

    pub const fn maximum(self) -> usize {
        match self {
            Self::Fixed(concurrency) => concurrency,
            Self::Adaptive { maximum, .. } => maximum,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Fixed(_) => "fixed",
            Self::Adaptive { .. } => "adaptive",
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct TransferStats {
    pub initial_concurrency: usize,
    pub maximum_concurrency: usize,
    pub peak_concurrency: usize,
    pub adjustment_count: usize,
    pub retry_count: usize,
    pub transferred_bytes: u64,
    pub total_bytes: u64,
    pub elapsed_seconds: f64,
    pub mib_per_second: f64,
}

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
    pub required_artifact_count: usize,
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
    pub resolved_artifact_file_count: usize,
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

/// A deterministic set of destructive mutations used by integration tests and benchmark lanes.
///
/// Paths are always interpreted relative to the reconstructed destination root. The command
/// adapter requires explicit confirmation before applying a plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconstructedCorruptionPlan {
    pub operations: Vec<ReconstructedCorruptionOperation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ReconstructedCorruptionOperation {
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
pub struct ReconstructedCorruptionRequest {
    pub destination_dir: PathBuf,
    pub plan: ReconstructedCorruptionPlan,
    pub control: GutchOperationControl,
}

#[derive(Debug, Clone)]
pub struct ReconstructedCorruptionResult {
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
    pub transfer_policy: TransferConcurrencyPolicy,
    pub control: GutchOperationControl,
}

#[derive(Debug, Clone)]
pub struct SnapshotFetchResult {
    pub repo_path: PathBuf,
    pub snapshot_id: String,
    pub required_bundle_count: usize,
    pub downloaded_bundle_count: usize,
    pub skipped_existing_bundle_count: usize,
    pub required_artifact_count: usize,
    pub downloaded_artifact_count: usize,
    pub skipped_existing_artifact_count: usize,
    pub transfer_policy: TransferConcurrencyPolicy,
    pub transfer_stats: TransferStats,
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

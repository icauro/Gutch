use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::manifest::launch::ResolvedLaunchConfiguration;
use crate::manifest::target::TargetManifest;
use crate::progress::GutchProgressSink;
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
pub struct ResolvedTargetInstallState {
    pub target_id: String,
    pub manifest_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch: Option<ResolvedLaunchConfiguration>,
}

#[derive(Debug, Clone)]
pub struct TargetInstallApplySource<'a> {
    pub target_manifest: &'a TargetManifest,
    pub install_dir: &'a Path,
}

#[derive(Debug, Clone)]
pub struct TargetInstallApplyResult {
    pub target_id: String,
    pub install_dir: PathBuf,
    pub staging_dir: PathBuf,
    pub backup_dir: Option<PathBuf>,
    pub reused_source_chunk_count: usize,
    pub required_chunk_count: usize,
    pub required_bundle_count: usize,
    pub reconstruct_timings: crate::ReconstructTimings,
    pub verify_timings: VerifyTimings,
}

#[derive(Debug, Clone)]
pub struct TargetInstallApplyFromRepoRequest {
    pub repo_path: PathBuf,
    pub target_id: String,
    pub install_dir: PathBuf,
    pub staging_dir: Option<PathBuf>,
    pub source_target_id: Option<String>,
    pub source_dir: Option<PathBuf>,
    pub runtime_hints: RuntimeResourceHints,
}

#[derive(Debug, Clone)]
pub struct TargetInstallApplyFromRepoResult {
    pub repo_root: PathBuf,
    pub target_manifest: TargetManifest,
    pub apply_result: TargetInstallApplyResult,
    pub resolved_bundle_file_count: usize,
    pub source_install_dir: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct TargetInstallVerifyFromRepoRequest {
    pub repo_path: PathBuf,
    pub target_id: String,
    pub install_dir: PathBuf,
    pub verify_mode: VerifyMode,
    pub runtime_hints: RuntimeResourceHints,
}

#[derive(Debug, Clone)]
pub struct TargetInstallVerifyFromRepoResult {
    pub repo_root: PathBuf,
    pub target_manifest: TargetManifest,
    pub classification: RepairClassification,
    pub verify_timings: VerifyTimings,
}

#[derive(Debug, Clone)]
pub struct TargetInstallRepairResult {
    pub target_id: String,
    pub install_dir: PathBuf,
    pub missing_file_count: usize,
    pub corrupt_file_count: usize,
    pub extra_file_count: usize,
    pub removed_extra_file_count: usize,
    pub restored_extra_file_count: usize,
    pub apply_result: Option<TargetInstallApplyResult>,
    pub verify_timings: VerifyTimings,
}

#[derive(Debug, Clone)]
pub struct TargetInstallRepairFromRepoRequest {
    pub repo_path: PathBuf,
    pub target_id: String,
    pub install_dir: PathBuf,
    pub staging_dir: Option<PathBuf>,
    pub remove_added_files: bool,
    pub runtime_hints: RuntimeResourceHints,
}

#[derive(Debug, Clone)]
pub struct TargetInstallRepairPlanFromRepoRequest {
    pub repo_path: PathBuf,
    pub target_id: String,
    pub install_dir: PathBuf,
    pub remove_added_files: bool,
    pub runtime_hints: RuntimeResourceHints,
}

#[derive(Debug, Clone)]
pub struct TargetInstallRepairPlanFromRepoResult {
    pub repo_root: PathBuf,
    pub target_manifest: TargetManifest,
    pub missing_file_count: usize,
    pub corrupt_file_count: usize,
    pub extra_file_count: usize,
    pub needs_rebuild: bool,
    pub required_bundle_ids: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct TargetInstallRepairFromRepoResult {
    pub repo_root: PathBuf,
    pub target_manifest: TargetManifest,
    pub repair_result: TargetInstallRepairResult,
    pub resolved_bundle_file_count: usize,
}

#[derive(Debug, Clone)]
pub struct TargetInstallFetchRequest {
    pub repo_path: PathBuf,
    pub target_id: String,
    pub repo_manifest_url: Option<String>,
    pub manifest_url: String,
    pub chunk_index_url: Option<String>,
    pub bundle_downloads_endpoint: Option<String>,
    pub session_id: Option<String>,
    pub bundles: Vec<GutchBundleDownload>,
    pub progress: Option<Arc<dyn GutchProgressSink>>,
}

#[derive(Debug, Clone)]
pub struct TargetInstallFetchResult {
    pub repo_path: PathBuf,
    pub target_id: String,
    pub required_bundle_count: usize,
    pub downloaded_bundle_count: usize,
    pub skipped_existing_bundle_count: usize,
}

#[derive(Debug, Clone)]
pub struct TargetInstallDownloadAndApplyRequest {
    pub fetch_request: TargetInstallFetchRequest,
    pub install_dir: PathBuf,
    pub staging_dir: Option<PathBuf>,
    pub source_target_id: Option<String>,
    pub source_dir: Option<PathBuf>,
    pub runtime_hints: RuntimeResourceHints,
}

#[derive(Debug, Clone)]
pub struct TargetInstallDownloadAndApplyResult {
    pub fetch_result: TargetInstallFetchResult,
    pub apply_result: TargetInstallApplyFromRepoResult,
}

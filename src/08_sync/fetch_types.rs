//! Requests and results for acquiring snapshot data into a repository.
use crate::progress::OperationControl;
use crate::transfer::types::{
    ArtifactDownload, ArtifactUrlProvider, TransferConcurrencyPolicy, TransferStats,
};
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone)]
pub struct SnapshotFetchRequest {
    pub repo_root: PathBuf,
    pub snapshot_id: String,
    pub repo_metadata_url: Option<String>,
    pub metadata_url: String,
    pub product_metadata_url: Option<String>,
    pub blueprint_url: String,
    pub chunk_index_url: Option<String>,
    pub artifacts: Vec<ArtifactDownload>,
    pub artifact_url_provider: Option<Arc<dyn ArtifactUrlProvider>>,
    pub transfer_policy: TransferConcurrencyPolicy,
    pub control: OperationControl,
}

/// Omits URLs, which may carry presigned credentials.
impl fmt::Debug for SnapshotFetchRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SnapshotFetchRequest")
            .field("repo_root", &self.repo_root)
            .field("snapshot_id", &self.snapshot_id)
            .field("artifact_count", &self.artifacts.len())
            .field(
                "has_artifact_url_provider",
                &self.artifact_url_provider.is_some(),
            )
            .field("transfer_policy", &self.transfer_policy)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone)]
pub struct SnapshotFetchResult {
    pub repo_root: PathBuf,
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

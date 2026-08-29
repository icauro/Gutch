use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use super::types::{BundleDownload, SnapshotFetchRequest};
use crate::manifest::gutch_repo::GutchRepoManifest;
use crate::progress::{
    GutchOperation, GutchOperationControl, GutchProgressEvent, GutchProgressPhase,
    GutchProgressSink, emit_progress,
};
use crate::{Error, Result};

const DOWNLOAD_PROGRESS_EMIT_BYTES: u64 = 256 * 1024;
const MAX_PARALLEL_BUNDLE_DOWNLOADS: usize = 4;
pub(super) const MAX_SNAPSHOT_METADATA_BYTES: u64 = 16 * 1024 * 1024;
pub(super) const MAX_SNAPSHOT_BLUEPRINT_BYTES: u64 = 256 * 1024 * 1024;
pub(super) const MAX_CHUNK_INDEX_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Default)]
pub(super) struct FetchTransferProgress {
    transferred_bytes: AtomicU64,
    total_bytes: AtomicU64,
    completed_bundle_count: AtomicUsize,
}

impl FetchTransferProgress {
    pub(super) fn add_total_bytes(&self, bytes: u64) -> u64 {
        self.total_bytes.fetch_add(bytes, Ordering::Relaxed) + bytes
    }

    pub(super) fn add_transferred_bytes(&self, bytes: u64) -> u64 {
        self.transferred_bytes.fetch_add(bytes, Ordering::Relaxed) + bytes
    }

    pub(super) fn mark_bundle_completed(&self) -> usize {
        self.completed_bundle_count.fetch_add(1, Ordering::Relaxed) + 1
    }

    pub(super) fn completed_bundle_count(&self) -> usize {
        self.completed_bundle_count.load(Ordering::Relaxed)
    }

    pub(super) fn transferred_bytes(&self) -> u64 {
        self.transferred_bytes.load(Ordering::Relaxed)
    }

    pub(super) fn total_bytes(&self) -> u64 {
        self.total_bytes.load(Ordering::Relaxed)
    }
}

#[derive(Clone, Copy)]
pub(super) struct TransferPhaseCounters {
    pub current: usize,
    pub total: usize,
}

pub(super) struct TransferDownloadRequest<'a> {
    pub(super) client: &'a reqwest::blocking::Client,
    pub(super) url: &'a str,
    pub(super) control: &'a GutchOperationControl,
    pub(super) transfer_progress: &'a Arc<FetchTransferProgress>,
    pub(super) phase: GutchProgressPhase,
    pub(super) message: &'a str,
    pub(super) counters: TransferPhaseCounters,
    pub(super) detail: Option<&'a str>,
}

impl<'a> TransferDownloadRequest<'a> {
    pub(super) fn download_bounded(self, maximum_bytes: u64) -> Result<Vec<u8>> {
        self.control.checkpoint()?;
        let mut bytes = Vec::new();
        let reporter = TransferProgressReporter::new(
            self.control,
            self.transfer_progress,
            self.phase,
            self.message,
            self.counters,
            self.detail,
        );
        crate::transport::download_url_streaming(
            self.client,
            self.url,
            self.control,
            Some(maximum_bytes),
            |content_length| {
                reporter.record_content_length(content_length);
                Ok(())
            },
            |chunk| {
                bytes.extend_from_slice(chunk);
                reporter.record_chunk(chunk.len() as u64);
                Ok(())
            },
        )?;
        reporter.finish();
        Ok(bytes)
    }

    pub(super) fn download_to_path(self, path: &Path) -> Result<()> {
        self.control.checkpoint()?;
        crate::helpers::write_atomic_with(path, |file| {
            let reporter = TransferProgressReporter::new(
                self.control,
                self.transfer_progress,
                self.phase,
                self.message,
                self.counters,
                self.detail,
            );
            crate::transport::download_url_streaming(
                self.client,
                self.url,
                self.control,
                None,
                |content_length| {
                    reporter.record_content_length(content_length);
                    Ok(())
                },
                |chunk| {
                    file.write_all(chunk)?;
                    reporter.record_chunk(chunk.len() as u64);
                    Ok(())
                },
            )?;
            reporter.finish();
            Ok(())
        })
    }
}

#[derive(Clone)]
pub(super) struct PendingBundleDownload {
    pub bundle_id: String,
    pub url: String,
    pub bundle_path: PathBuf,
    pub ordinal: usize,
    pub total_missing_bundle_count: usize,
}

pub(super) use crate::transport::build_download_client;

struct TransferProgressReporter<'a> {
    control: &'a GutchOperationControl,
    transfer_progress: &'a Arc<FetchTransferProgress>,
    phase: GutchProgressPhase,
    message: &'a str,
    counters: TransferPhaseCounters,
    detail: Option<&'a str>,
    emitted_chunk_bytes: AtomicU64,
}

impl<'a> TransferProgressReporter<'a> {
    fn new(
        control: &'a GutchOperationControl,
        transfer_progress: &'a Arc<FetchTransferProgress>,
        phase: GutchProgressPhase,
        message: &'a str,
        counters: TransferPhaseCounters,
        detail: Option<&'a str>,
    ) -> Self {
        Self {
            control,
            transfer_progress,
            phase,
            message,
            counters,
            detail,
            emitted_chunk_bytes: AtomicU64::new(0),
        }
    }

    fn record_content_length(&self, content_length: Option<u64>) {
        if let Some(content_length) = content_length {
            self.transfer_progress.add_total_bytes(content_length);
        }
    }

    fn record_chunk(&self, byte_count: u64) {
        self.transfer_progress.add_transferred_bytes(byte_count);
        let emitted_chunk_bytes = self
            .emitted_chunk_bytes
            .fetch_add(byte_count, Ordering::Relaxed)
            + byte_count;
        if emitted_chunk_bytes >= DOWNLOAD_PROGRESS_EMIT_BYTES {
            emit_transfer_progress(
                self.control.progress(),
                self.transfer_progress,
                self.phase,
                self.message,
                self.counters,
                self.detail,
                false,
            );
            self.emitted_chunk_bytes.store(0, Ordering::Relaxed);
        }
    }

    fn finish(&self) {
        emit_progress(
            self.control.progress(),
            GutchProgressEvent {
                operation: GutchOperation::Fetch,
                phase: self.phase,
                message: "Downloaded snapshot bundles".to_string(),
                current: self
                    .counters
                    .total
                    .min(self.counters.current.saturating_add(1)),
                total: self.counters.total.max(1),
                transferred_bytes: self.transfer_progress.transferred_bytes(),
                total_bytes: self.transfer_progress.total_bytes(),
                detail: self.detail.map(str::to_string),
                done: false,
            },
        );
    }
}

pub(super) fn bundle_download_worker_count(pending_bundle_count: usize) -> usize {
    let available_workers = std::thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(1)
        .max(1);
    pending_bundle_count
        .clamp(1, MAX_PARALLEL_BUNDLE_DOWNLOADS)
        .min(available_workers)
}

pub(super) fn normalize_bundle_downloads(
    bundles: &[BundleDownload],
) -> Result<HashMap<String, String>> {
    let mut normalized = HashMap::with_capacity(bundles.len());
    for bundle in bundles {
        let bundle_id = non_empty_trimmed(&bundle.bundle_id, "bundle id")?;
        let url = non_empty_trimmed(&bundle.url, "bundle URL")?;
        if normalized
            .insert(bundle_id.to_string(), url.to_string())
            .is_some()
        {
            return Err(Error::manifest(format!(
                "duplicate direct bundle URL for '{}'",
                bundle_id
            )));
        }
    }
    Ok(normalized)
}

pub(super) fn fetch_bundle_urls(
    request: &SnapshotFetchRequest,
    required_bundle_ids: &std::collections::HashSet<String>,
    existing_bundle_ids: &std::collections::HashSet<String>,
) -> Result<HashMap<String, String>> {
    let mut bundle_urls = normalize_bundle_downloads(&request.bundles)?;
    let unresolved_bundle_ids = required_bundle_ids
        .iter()
        .filter(|bundle_id| {
            !existing_bundle_ids.contains(*bundle_id)
                && !bundle_urls.contains_key(bundle_id.as_str())
        })
        .cloned()
        .collect::<Vec<_>>();
    if unresolved_bundle_ids.is_empty() {
        return Ok(bundle_urls);
    }

    let provider = request.bundle_url_provider.as_ref().ok_or_else(|| {
        Error::manifest("bundle URL provider is required when direct bundle URLs are not provided")
    })?;
    let resolved_bundles =
        provider.resolve_bundle_urls(&unresolved_bundle_ids, &request.control)?;
    for bundle in resolved_bundles {
        let bundle_id = non_empty_trimmed(&bundle.bundle_id, "bundle id")?;
        let url = non_empty_trimmed(&bundle.url, "bundle URL")?;
        if !unresolved_bundle_ids.iter().any(|id| id == bundle_id) {
            return Err(Error::manifest(format!(
                "bundle URL provider returned unexpected bundle '{}'",
                bundle_id
            )));
        }
        if bundle_urls
            .insert(bundle_id.to_string(), url.to_string())
            .is_some()
        {
            return Err(Error::manifest(format!(
                "bundle URL provider returned duplicate bundle '{}'",
                bundle_id
            )));
        }
    }

    let missing = unresolved_bundle_ids
        .into_iter()
        .filter(|bundle_id| !bundle_urls.contains_key(bundle_id))
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(Error::manifest(format!(
            "missing bundle download URLs for: {}",
            missing.join(", ")
        )));
    }
    Ok(bundle_urls)
}

pub(super) fn write_repo_manifest(
    repo_paths: &crate::GutchRepoPaths,
    repo_manifest: &GutchRepoManifest,
) -> Result<()> {
    crate::write_gutch_repo_manifest(repo_paths, repo_manifest)
}

pub fn snapshot_manifest_from_download(
    snapshot_id: &str,
    manifest_bytes: &[u8],
) -> Result<crate::manifest::snapshot::SnapshotManifest> {
    let metadata_json = std::str::from_utf8(manifest_bytes)
        .map_err(|err| Error::manifest(format!("snapshot metadata is not valid UTF-8: {}", err)))?;
    let mut metadata =
        serde_json::from_str::<crate::manifest::snapshot::SnapshotMetadata>(metadata_json)?;
    metadata.snapshot_id = snapshot_id.to_string();
    Ok(crate::manifest::snapshot::SnapshotManifest::from_metadata(
        metadata,
    ))
}

pub(super) fn download_pending_bundle(
    download_client: &reqwest::blocking::Client,
    bundle_download: &PendingBundleDownload,
    control: &GutchOperationControl,
    transfer_progress: &Arc<FetchTransferProgress>,
) -> Result<usize> {
    control.checkpoint()?;
    if let Some(parent) = bundle_download.bundle_path.parent() {
        crate::helpers::create_dir_all(parent)?;
    }
    let message = format!(
        "Downloading bundles ({}/{})",
        bundle_download.ordinal + 1,
        bundle_download.total_missing_bundle_count.max(1)
    );
    TransferDownloadRequest {
        client: download_client,
        url: &bundle_download.url,
        control,
        transfer_progress,
        phase: GutchProgressPhase::Bundle,
        message: &message,
        counters: TransferPhaseCounters {
            current: transfer_progress.completed_bundle_count(),
            total: bundle_download.total_missing_bundle_count.max(1),
        },
        detail: Some(bundle_download.bundle_id.as_str()),
    }
    .download_to_path(&bundle_download.bundle_path)?;
    Ok(transfer_progress.mark_bundle_completed())
}

fn emit_transfer_progress(
    progress: &Option<Arc<dyn GutchProgressSink>>,
    transfer_progress: &Arc<FetchTransferProgress>,
    phase: GutchProgressPhase,
    message: &str,
    counters: TransferPhaseCounters,
    detail: Option<&str>,
    done: bool,
) {
    emit_progress(
        progress,
        GutchProgressEvent {
            operation: GutchOperation::Fetch,
            phase,
            message: message.to_string(),
            current: counters.current,
            total: counters.total.max(1),
            transferred_bytes: transfer_progress.transferred_bytes(),
            total_bytes: transfer_progress.total_bytes(),
            detail: detail.map(str::to_string),
            done,
        },
    );
}

fn non_empty_trimmed<'a>(value: &'a str, label: &str) -> Result<&'a str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(Error::manifest(format!("{label} is required")));
    }
    Ok(trimmed)
}

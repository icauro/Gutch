use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

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

#[derive(Clone)]
pub(super) struct PendingBundleDownload {
    pub bundle_id: String,
    pub url: String,
    pub bundle_path: PathBuf,
    pub ordinal: usize,
    pub total_missing_bundle_count: usize,
}

pub(super) fn bundle_download_worker_count(pending_bundle_count: usize) -> usize {
    let available_workers = std::thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(1)
        .max(1);
    pending_bundle_count
        .max(1)
        .min(MAX_PARALLEL_BUNDLE_DOWNLOADS)
        .min(available_workers)
}

pub(super) fn build_download_client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .user_agent("Gutch CLI")
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30 * 60))
        .build()
        .map_err(|err| Error::manifest(format!("failed to create download client: {}", err)))
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
    download_url_to_path(
        download_client,
        &bundle_download.url,
        &bundle_download.bundle_path,
        control,
        transfer_progress,
        GutchProgressPhase::Bundle,
        &format!(
            "Downloading bundles ({}/{})",
            bundle_download.ordinal + 1,
            bundle_download.total_missing_bundle_count.max(1)
        ),
        TransferPhaseCounters {
            current: transfer_progress.completed_bundle_count(),
            total: bundle_download.total_missing_bundle_count.max(1),
        },
        Some(bundle_download.bundle_id.as_str()),
    )?;
    Ok(transfer_progress.mark_bundle_completed())
}

pub(super) fn download_url_bounded(
    client: &reqwest::blocking::Client,
    url: &str,
    control: &GutchOperationControl,
    transfer_progress: &Arc<FetchTransferProgress>,
    phase: GutchProgressPhase,
    message: &str,
    counters: TransferPhaseCounters,
    detail: Option<&str>,
    maximum_bytes: u64,
) -> Result<Vec<u8>> {
    download_url_with_limit(
        client,
        url,
        control,
        transfer_progress,
        phase,
        message,
        counters,
        detail,
        Some(maximum_bytes),
    )
}

fn download_url_with_limit(
    client: &reqwest::blocking::Client,
    url: &str,
    control: &GutchOperationControl,
    transfer_progress: &Arc<FetchTransferProgress>,
    phase: GutchProgressPhase,
    message: &str,
    counters: TransferPhaseCounters,
    detail: Option<&str>,
    maximum_bytes: Option<u64>,
) -> Result<Vec<u8>> {
    control.checkpoint()?;
    let mut bytes = Vec::new();
    download_url_streaming(
        client,
        url,
        control,
        transfer_progress,
        phase,
        message,
        counters,
        detail,
        maximum_bytes,
        |chunk| {
            if let Some(maximum) = maximum_bytes
                && bytes.len().saturating_add(chunk.len()) as u64 > maximum
            {
                return Err(Error::manifest("download exceeded the allowed size"));
            }
            bytes.extend_from_slice(chunk);
            Ok(())
        },
    )?;
    Ok(bytes)
}

pub(super) fn download_url_to_path(
    client: &reqwest::blocking::Client,
    url: &str,
    path: &Path,
    control: &GutchOperationControl,
    transfer_progress: &Arc<FetchTransferProgress>,
    phase: GutchProgressPhase,
    message: &str,
    counters: TransferPhaseCounters,
    detail: Option<&str>,
) -> Result<()> {
    control.checkpoint()?;
    let temporary_path = path.with_extension("downloading");
    if temporary_path.exists() {
        let _ = std::fs::remove_file(&temporary_path);
    }
    let result = (|| -> Result<()> {
        let mut file = File::create(&temporary_path)?;
        download_url_streaming(
            client,
            url,
            control,
            transfer_progress,
            phase,
            message,
            counters,
            detail,
            None,
            |chunk| {
                file.write_all(chunk)?;
                Ok(())
            },
        )?;
        file.flush()?;
        drop(file);
        control.checkpoint()?;
        std::fs::rename(&temporary_path, path)?;
        Ok(())
    })();

    if result.is_err() {
        let _ = std::fs::remove_file(&temporary_path);
    }
    result
}

fn download_url_streaming<F>(
    client: &reqwest::blocking::Client,
    url: &str,
    control: &GutchOperationControl,
    transfer_progress: &Arc<FetchTransferProgress>,
    phase: GutchProgressPhase,
    message: &str,
    counters: TransferPhaseCounters,
    detail: Option<&str>,
    maximum_bytes: Option<u64>,
    mut on_chunk: F,
) -> Result<()>
where
    F: FnMut(&[u8]) -> Result<()>,
{
    control.checkpoint()?;
    let response = client
        .get(url)
        .send()
        .map_err(|err| Error::manifest(format!("download request failed: {}", err)))?;
    if !response.status().is_success() {
        return Err(Error::manifest(format!(
            "download request failed with status {}",
            response.status()
        )));
    }
    if maximum_bytes.is_some_and(|maximum| {
        response
            .content_length()
            .is_some_and(|length| length > maximum)
    }) {
        return Err(Error::manifest("download exceeded the allowed size"));
    }
    if let Some(content_length) = response.content_length() {
        transfer_progress.add_total_bytes(content_length);
    }
    let mut response = response;
    let mut buffer = [0u8; 64 * 1024];
    let mut downloaded_bytes = 0_u64;
    let mut emitted_chunk_bytes = 0u64;
    loop {
        control.checkpoint()?;
        let read = response
            .read(&mut buffer)
            .map_err(|err| Error::manifest(format!("failed to read download response: {}", err)))?;
        if read == 0 {
            break;
        }
        downloaded_bytes = downloaded_bytes.saturating_add(read as u64);
        if maximum_bytes.is_some_and(|maximum| downloaded_bytes > maximum) {
            return Err(Error::manifest("download exceeded the allowed size"));
        }
        on_chunk(&buffer[..read])?;
        transfer_progress.add_transferred_bytes(read as u64);
        emitted_chunk_bytes += read as u64;
        if emitted_chunk_bytes >= DOWNLOAD_PROGRESS_EMIT_BYTES {
            emit_transfer_progress(
                control.progress(),
                transfer_progress,
                phase,
                message,
                counters,
                detail,
                false,
            );
            emitted_chunk_bytes = 0;
        }
    }
    emit_progress(
        control.progress(),
        GutchProgressEvent {
            operation: GutchOperation::Fetch,
            phase,
            message: "Downloaded snapshot bundles".to_string(),
            current: counters.total.min(counters.current.saturating_add(1)),
            total: counters.total.max(1),
            transferred_bytes: transfer_progress.transferred_bytes(),
            total_bytes: transfer_progress.total_bytes(),
            detail: detail.map(str::to_string),
            done: false,
        },
    );
    Ok(())
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

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use serde::Deserialize;

use super::types::{GutchBundleDownload, TargetInstallFetchRequest};
use crate::manifest::gutch_repo::GutchRepoManifest;
use crate::progress::{
    GutchOperation, GutchOperationControl, GutchProgressEvent, GutchProgressPhase,
    GutchProgressSink, emit_progress,
};
use crate::{Error, Result};

const DOWNLOAD_PROGRESS_EMIT_BYTES: u64 = 256 * 1024;
const MAX_PARALLEL_BUNDLE_DOWNLOADS: usize = 4;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GutchBundleUrlsResponse {
    #[serde(default)]
    bundles: Vec<GutchBundleDownload>,
}

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
        .build()
        .map_err(|err| Error::manifest(format!("failed to create download client: {}", err)))
}

pub(super) fn normalize_bundle_downloads(
    bundles: &[GutchBundleDownload],
) -> Result<HashMap<String, String>> {
    let mut normalized = HashMap::with_capacity(bundles.len());
    for bundle in bundles {
        let bundle_id = non_empty_trimmed(&bundle.bundle_id, "bundle id")?;
        let url = non_empty_trimmed(&bundle.url, "bundle URL")?;
        normalized.insert(bundle_id.to_string(), url.to_string());
    }
    Ok(normalized)
}

pub(super) fn fetch_bundle_urls(
    request: &TargetInstallFetchRequest,
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

    let endpoint = request
        .bundle_downloads_endpoint
        .as_deref()
        .ok_or_else(|| {
            Error::manifest(
                "bundle-downloads-endpoint is required when direct bundle URLs are not provided",
            )
        })?;
    let ids = unresolved_bundle_ids.join(",");
    let separator = if endpoint.contains('?') { "&" } else { "?" };
    let response_bytes = download_json_bytes(
        &format!("{endpoint}{separator}ids={ids}"),
        request.session_id.as_deref(),
        &request.control,
    )?;
    let response: GutchBundleUrlsResponse = crate::helpers::from_json_bytes(&response_bytes)?;
    for bundle in response.bundles {
        let bundle_id = non_empty_trimmed(&bundle.bundle_id, "bundle id")?;
        let url = non_empty_trimmed(&bundle.url, "bundle URL")?;
        bundle_urls.insert(bundle_id.to_string(), url.to_string());
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

pub(super) fn target_manifest_from_download(
    repo_paths: &crate::GutchRepoPaths,
    target_id: &str,
    manifest_bytes: &[u8],
) -> Result<crate::manifest::target::TargetManifest> {
    let manifest_json = std::str::from_utf8(manifest_bytes)
        .map_err(|err| Error::manifest(format!("target manifest is not valid UTF-8: {}", err)))?;
    let manifest_value: serde_json::Value = serde_json::from_str(manifest_json)?;
    let mut manifest = if manifest_value.get("files").is_some() {
        serde_json::from_value::<crate::manifest::target::TargetManifest>(manifest_value)?
    } else {
        let shell: crate::manifest::binary::TargetManifestShell =
            serde_json::from_str(manifest_json)?;
        crate::manifest::binary::target_manifest_from_shell(
            shell,
            target_id.to_string(),
            Vec::new(),
        )
    };
    if manifest.target_id.trim().is_empty() || manifest.target_id != target_id {
        manifest.target_id = target_id.to_string();
    }
    let _ = repo_paths;
    Ok(manifest)
}

pub(super) fn download_pending_bundle(
    download_client: &reqwest::blocking::Client,
    bundle_download: &PendingBundleDownload,
    session_id: Option<&str>,
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
        session_id,
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

pub(super) fn download_url(
    client: &reqwest::blocking::Client,
    url: &str,
    session_id: Option<&str>,
    control: &GutchOperationControl,
    transfer_progress: &Arc<FetchTransferProgress>,
    phase: GutchProgressPhase,
    message: &str,
    counters: TransferPhaseCounters,
    detail: Option<&str>,
) -> Result<Vec<u8>> {
    control.checkpoint()?;
    let mut bytes = Vec::new();
    download_url_streaming(
        client,
        url,
        session_id,
        control,
        transfer_progress,
        phase,
        message,
        counters,
        detail,
        |chunk| {
            bytes.extend_from_slice(chunk);
            Ok(())
        },
    )?;
    Ok(bytes)
}

pub(super) fn download_url_to_path(
    client: &reqwest::blocking::Client,
    url: &str,
    session_id: Option<&str>,
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
            session_id,
            control,
            transfer_progress,
            phase,
            message,
            counters,
            detail,
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
    session_id: Option<&str>,
    control: &GutchOperationControl,
    transfer_progress: &Arc<FetchTransferProgress>,
    phase: GutchProgressPhase,
    message: &str,
    counters: TransferPhaseCounters,
    detail: Option<&str>,
    mut on_chunk: F,
) -> Result<()>
where
    F: FnMut(&[u8]) -> Result<()>,
{
    control.checkpoint()?;
    let mut request = client.get(url);
    if should_attach_bearer_session(url, session_id) {
        request = request.header(
            "Authorization",
            format!("Bearer {}", session_id.expect("session already checked")),
        );
    }
    let response = request
        .send()
        .map_err(|err| Error::manifest(format!("download request failed: {}", err)))?;
    if !response.status().is_success() {
        return Err(Error::manifest(format!(
            "download request failed with status {}",
            response.status()
        )));
    }
    if let Some(content_length) = response.content_length() {
        transfer_progress.add_total_bytes(content_length);
    }
    let mut response = response;
    let mut buffer = [0u8; 64 * 1024];
    let mut emitted_chunk_bytes = 0u64;
    loop {
        control.checkpoint()?;
        let read = response
            .read(&mut buffer)
            .map_err(|err| Error::manifest(format!("failed to read download response: {}", err)))?;
        if read == 0 {
            break;
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
            message: "Downloaded content".to_string(),
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

fn should_attach_bearer_session(url: &str, session_id: Option<&str>) -> bool {
    session_id.is_some() && !url.contains("X-Amz-Algorithm=") && !url.contains("sig=")
}

fn download_json_bytes(
    url: &str,
    session_id: Option<&str>,
    control: &GutchOperationControl,
) -> Result<Vec<u8>> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("Gutch CLI")
        .build()
        .map_err(|err| Error::manifest(format!("failed to create download client: {}", err)))?;
    download_url(
        &client,
        url,
        session_id,
        control,
        &Arc::new(FetchTransferProgress::default()),
        GutchProgressPhase::Json,
        "Downloading JSON",
        TransferPhaseCounters {
            current: 0,
            total: 1,
        },
        None,
    )
}

fn non_empty_trimmed<'a>(value: &'a str, label: &str) -> Result<&'a str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(Error::manifest(format!("{label} is required")));
    }
    Ok(trimmed)
}

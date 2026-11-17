//! Stream one artifact with bounded retry and cancellation.
use super::download::PendingArtifactDownload;
use super::progress::{FetchTransferProgress, TransferPhaseCounters, TransferProgressReporter};
use crate::Result;
use crate::artifact::bundle::verify_bundle_identity_reader;
use crate::artifact::reference::ArtifactRef;
use crate::artifact::seal::SealKeys;
use crate::filesystem;
use crate::filesystem::write_atomic_with;
use crate::progress::{OperationControl, ProgressPhase};
use crate::transfer::http::DownloadClient;
use crate::transfer::http::download_url_streaming;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
const MAX_DOWNLOAD_ATTEMPTS: usize = 3;
const RETRY_BASE_DELAY: Duration = Duration::from_millis(250);
const RETRY_SLEEP_SLICE: Duration = Duration::from_millis(100);

/// Largest accepted artifact body: the 128 MiB bundle preset can overshoot by
/// one 16 MiB chunk plus its tables, so this leaves ample headroom while still
/// bounding a hostile or misconfigured source.
const MAX_ARTIFACT_DOWNLOAD_BYTES: u64 = 256 * 1024 * 1024;

pub(crate) struct TransferDownloadRequest<'a> {
    pub(crate) client: &'a DownloadClient,
    pub(crate) url: &'a str,
    pub(crate) control: &'a OperationControl,
    pub(crate) transfer_progress: &'a Arc<FetchTransferProgress>,
    pub(crate) phase: ProgressPhase,
    pub(crate) message: &'a str,
    pub(crate) counters: TransferPhaseCounters,
    pub(crate) detail: Option<&'a str>,
}

impl TransferDownloadRequest<'_> {
    /// Downloads into memory, retrying transient failures.
    pub(crate) fn download_bounded(&self, maximum_bytes: u64) -> Result<Vec<u8>> {
        self.with_retries(|| {
            let reporter = self.reporter();
            let mut bytes = Vec::new();
            self.stream(&reporter, maximum_bytes, |chunk| {
                bytes.extend_from_slice(chunk);
                Ok(())
            })?;
            reporter.finish();
            Ok(bytes)
        })
    }

    /// Downloads into `path` atomically, retrying transient failures.
    /// A bundle is checked against its ID; `keys` are the repository's when
    /// it is encrypted.
    pub(crate) fn download_to_path(
        &self,
        path: &Path,
        artifact: &ArtifactRef,
        keys: Option<&SealKeys>,
    ) -> Result<()> {
        self.with_retries(|| {
            write_atomic_with(path, |file| {
                let reporter = self.reporter();
                self.stream(&reporter, MAX_ARTIFACT_DOWNLOAD_BYTES, |chunk| {
                    file.write_all(chunk)?;
                    Ok(())
                })?;
                if let ArtifactRef::Bundle { id } = artifact {
                    verify_bundle_identity_reader(file, id, keys)?;
                }
                reporter.finish();
                Ok(())
            })
        })
    }

    fn reporter(&self) -> TransferProgressReporter<'_> {
        TransferProgressReporter::new(
            self.control,
            self.transfer_progress,
            self.phase,
            self.message,
            self.counters,
            self.detail,
        )
    }

    fn stream(
        &self,
        reporter: &TransferProgressReporter<'_>,
        maximum_bytes: u64,
        mut on_chunk: impl FnMut(&[u8]) -> Result<()>,
    ) -> Result<()> {
        download_url_streaming(
            self.client,
            self.url,
            self.control,
            maximum_bytes,
            |content_length| {
                reporter.record_content_length(content_length);
                Ok(())
            },
            |chunk| {
                reporter.record_chunk(chunk.len() as u64);
                on_chunk(chunk)
            },
        )
    }

    fn with_retries<T>(&self, mut attempt: impl FnMut() -> Result<T>) -> Result<T> {
        let mut attempt_number = 1;
        loop {
            self.control.checkpoint()?;
            match attempt() {
                Err(error)
                    if attempt_number < MAX_DOWNLOAD_ATTEMPTS && error.is_retryable_download() =>
                {
                    self.transfer_progress.retry_recorded();
                    sleep_with_checkpoint(self.control, retry_delay(attempt_number))?;
                    attempt_number += 1;
                }
                result => return result,
            }
        }
    }
}

pub(crate) fn download_pending_artifact(
    download_client: &DownloadClient,
    artifact_download: &PendingArtifactDownload,
    control: &OperationControl,
    transfer_progress: &Arc<FetchTransferProgress>,
) -> Result<()> {
    control.checkpoint()?;
    let _artifact_span = control
        .profile_span_with_args("fetch.artifact.transfer", || artifact_download.trace_args());
    if let Some(parent) = artifact_download.artifact_path.parent() {
        filesystem::create_dir_all(parent)?;
    }
    transfer_progress.download_started();
    let message = format!(
        "Downloading artifacts ({}/{})",
        artifact_download.ordinal + 1,
        artifact_download.missing_artifact_count.max(1)
    );
    let result = TransferDownloadRequest {
        client: download_client,
        url: &artifact_download.url,
        control,
        transfer_progress,
        phase: ProgressPhase::Artifact,
        message: &message,
        counters: TransferPhaseCounters {
            current: transfer_progress.completed_artifact_count(),
            total: artifact_download.missing_artifact_count.max(1),
        },
        detail: Some(artifact_download.artifact_id.as_str()),
    }
    .download_to_path(
        &artifact_download.artifact_path,
        &artifact_download.artifact,
        artifact_download.keys.as_deref(),
    );
    transfer_progress.download_finished();
    result?;
    transfer_progress.mark_artifact_completed();
    Ok(())
}

/// Doubles from the base delay: 250 ms, then 500 ms.
fn retry_delay(attempt_number: usize) -> Duration {
    RETRY_BASE_DELAY.saturating_mul(1 << attempt_number.saturating_sub(1).min(16))
}

fn sleep_with_checkpoint(control: &OperationControl, delay: Duration) -> Result<()> {
    let deadline = Instant::now() + delay;
    loop {
        control.checkpoint()?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(());
        }
        thread::sleep(remaining.min(RETRY_SLEEP_SLICE));
    }
}

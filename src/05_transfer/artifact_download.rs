//! Stream one artifact with bounded retry and cancellation.
use super::download::PendingArtifactDownload;
use super::progress::{FetchTransferProgress, TransferPhaseCounters, TransferProgressReporter};
use crate::Result;
use crate::artifact::bundle::verify_bundle_identity_reader;
use crate::artifact::reference::ArtifactRef;
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
const MAX_ARTIFACT_DOWNLOAD_ATTEMPTS: usize = 3;

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

impl<'a> TransferDownloadRequest<'a> {
    pub(crate) fn download_bounded(self, maximum_bytes: u64) -> Result<Vec<u8>> {
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
        download_url_streaming(
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

    pub(crate) fn download_to_path(self, path: &Path, artifact: &ArtifactRef) -> Result<()> {
        self.control.checkpoint()?;
        write_atomic_with(path, |file| {
            let reporter = TransferProgressReporter::new(
                self.control,
                self.transfer_progress,
                self.phase,
                self.message,
                self.counters,
                self.detail,
            );
            download_url_streaming(
                self.client,
                self.url,
                self.control,
                Some(MAX_ARTIFACT_DOWNLOAD_BYTES),
                |content_length| {
                    reporter.record_content_length(content_length);
                    Ok(())
                },
                |chunk| {
                    reporter.record_chunk(chunk.len() as u64);
                    file.write_all(chunk)?;
                    Ok(())
                },
            )?;
            if let ArtifactRef::Bundle { id } = artifact {
                verify_bundle_identity_reader(file, id)?;
            }
            reporter.finish();
            Ok(())
        })
    }
}

pub(crate) fn download_pending_artifact(
    download_client: &DownloadClient,
    artifact_download: &PendingArtifactDownload,
    control: &OperationControl,
    transfer_progress: &Arc<FetchTransferProgress>,
) -> Result<usize> {
    control.checkpoint()?;
    let _artifact_span = control
        .profile_span_with_args("fetch.artifact.transfer", || artifact_download.trace_args());
    if let Some(parent) = artifact_download.artifact_path.parent() {
        filesystem::create_dir_all(parent)?;
    }
    transfer_progress.download_started();
    let result = (|| -> Result<()> {
        let message = format!(
            "Downloading artifacts ({}/{})",
            artifact_download.ordinal + 1,
            artifact_download.missing_artifact_count.max(1)
        );
        for attempt in 1..=MAX_ARTIFACT_DOWNLOAD_ATTEMPTS {
            control.checkpoint()?;
            let attempt_result = TransferDownloadRequest {
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
            );
            match attempt_result {
                Ok(()) => return Ok(()),
                Err(error)
                    if attempt < MAX_ARTIFACT_DOWNLOAD_ATTEMPTS
                        && error.is_retryable_download() =>
                {
                    transfer_progress.retry_recorded();
                    sleep_with_checkpoint(control, retry_delay(attempt))?;
                }
                Err(error) => return Err(error),
            }
        }
        unreachable!("artifact download attempt loop must return")
    })();
    transfer_progress.download_finished();
    result?;
    Ok(transfer_progress.mark_artifact_completed())
}

fn retry_delay(attempt: usize) -> Duration {
    Duration::from_millis(250_u64.saturating_mul(1_u64 << attempt.saturating_sub(1)))
}

fn sleep_with_checkpoint(control: &OperationControl, delay: Duration) -> Result<()> {
    let deadline = Instant::now() + delay;
    loop {
        control.checkpoint()?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(());
        }
        thread::sleep(remaining.min(Duration::from_millis(100)));
    }
}

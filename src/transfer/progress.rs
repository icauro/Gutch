//! Shared transfer counters and live feedback.
use super::types::{TransferConcurrencyPolicy, TransferStats};
use crate::progress::{
    GutchOperation, GutchOperationControl, GutchProgressEvent, GutchProgressPhase,
    GutchProgressSink, emit_progress,
};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
const DOWNLOAD_PROGRESS_EMIT_BYTES: u64 = 256 * 1024;

#[derive(Debug)]
pub(crate) struct FetchTransferProgress {
    started_at: Instant,
    finished_at: Mutex<Option<Instant>>,
    transferred_bytes: AtomicU64,
    total_bytes: AtomicU64,
    completed_artifact_count: AtomicUsize,
    active_download_count: AtomicUsize,
    peak_download_count: AtomicUsize,
    retry_count: AtomicUsize,
    adjustment_count: AtomicUsize,
    downstream_backpressure: AtomicBool,
}

impl Default for FetchTransferProgress {
    fn default() -> Self {
        Self {
            started_at: Instant::now(),
            finished_at: Mutex::new(None),
            transferred_bytes: AtomicU64::default(),
            total_bytes: AtomicU64::default(),
            completed_artifact_count: AtomicUsize::default(),
            active_download_count: AtomicUsize::default(),
            peak_download_count: AtomicUsize::default(),
            retry_count: AtomicUsize::default(),
            adjustment_count: AtomicUsize::default(),
            downstream_backpressure: AtomicBool::default(),
        }
    }
}

impl FetchTransferProgress {
    pub(crate) fn add_total_bytes(&self, bytes: u64) -> u64 {
        self.total_bytes.fetch_add(bytes, AtomicOrdering::Relaxed) + bytes
    }

    pub(crate) fn add_transferred_bytes(&self, bytes: u64) -> u64 {
        self.transferred_bytes
            .fetch_add(bytes, AtomicOrdering::Relaxed)
            + bytes
    }

    pub(crate) fn mark_artifact_completed(&self) -> usize {
        self.completed_artifact_count
            .fetch_add(1, AtomicOrdering::Relaxed)
            + 1
    }

    pub(crate) fn completed_artifact_count(&self) -> usize {
        self.completed_artifact_count.load(AtomicOrdering::Relaxed)
    }

    pub(crate) fn transferred_bytes(&self) -> u64 {
        self.transferred_bytes.load(AtomicOrdering::Relaxed)
    }

    pub(crate) fn total_bytes(&self) -> u64 {
        self.total_bytes.load(AtomicOrdering::Relaxed)
    }

    pub(crate) fn finish(&self) {
        if let Ok(mut finished_at) = self.finished_at.lock() {
            if finished_at.is_none() {
                *finished_at = Some(Instant::now());
            }
        }
    }

    pub(super) fn download_started(&self) {
        let active = self
            .active_download_count
            .fetch_add(1, AtomicOrdering::AcqRel)
            + 1;
        let mut peak = self.peak_download_count.load(AtomicOrdering::Acquire);
        while active > peak {
            match self.peak_download_count.compare_exchange_weak(
                peak,
                active,
                AtomicOrdering::AcqRel,
                AtomicOrdering::Acquire,
            ) {
                Ok(_) => break,
                Err(observed) => peak = observed,
            }
        }
    }

    pub(super) fn download_finished(&self) {
        self.active_download_count
            .fetch_sub(1, AtomicOrdering::AcqRel);
    }

    pub(super) fn retry_recorded(&self) {
        self.retry_count.fetch_add(1, AtomicOrdering::Relaxed);
    }

    pub(crate) fn adjustment_recorded(&self) {
        self.adjustment_count.fetch_add(1, AtomicOrdering::Relaxed);
    }

    pub(crate) fn set_downstream_backpressure(&self, enabled: bool) {
        self.downstream_backpressure
            .store(enabled, AtomicOrdering::Release);
    }

    pub(super) fn downstream_backpressure(&self) -> bool {
        self.downstream_backpressure.load(AtomicOrdering::Acquire)
    }

    pub(crate) fn retry_count(&self) -> usize {
        self.retry_count.load(AtomicOrdering::Acquire)
    }

    pub(crate) fn stats(&self, policy: TransferConcurrencyPolicy) -> TransferStats {
        let transferred_bytes = self.transferred_bytes();
        let total_bytes = self.total_bytes();
        let finished_at = self
            .finished_at
            .lock()
            .ok()
            .and_then(|finished_at| *finished_at)
            .unwrap_or_else(Instant::now);
        let elapsed_seconds = finished_at
            .saturating_duration_since(self.started_at)
            .as_secs_f64();
        TransferStats {
            initial_concurrency: policy.initial(),
            maximum_concurrency: policy.maximum(),
            peak_concurrency: self.peak_download_count.load(AtomicOrdering::Acquire),
            adjustment_count: self.adjustment_count.load(AtomicOrdering::Acquire),
            retry_count: self.retry_count.load(AtomicOrdering::Acquire),
            transferred_bytes,
            total_bytes,
            elapsed_seconds,
            mib_per_second: if elapsed_seconds > f64::EPSILON {
                transferred_bytes as f64 / (1024.0 * 1024.0) / elapsed_seconds
            } else {
                0.0
            },
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct TransferPhaseCounters {
    pub current: usize,
    pub total: usize,
}

pub(super) struct TransferProgressReporter<'a> {
    control: &'a GutchOperationControl,
    transfer_progress: &'a Arc<FetchTransferProgress>,
    phase: GutchProgressPhase,
    message: &'a str,
    counters: TransferPhaseCounters,
    detail: Option<&'a str>,
    emitted_chunk_bytes: AtomicU64,
}

impl<'a> TransferProgressReporter<'a> {
    pub(super) fn new(
        control: &'a GutchOperationControl,
        transfer_progress: &'a Arc<FetchTransferProgress>,
        phase: GutchProgressPhase,
        message: &'a str,
        counters: TransferPhaseCounters,
        detail: Option<&'a str>,
    ) -> Self {
        control.download_started();
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

    pub(super) fn record_content_length(&self, content_length: Option<u64>) {
        if let Some(content_length) = content_length {
            self.transfer_progress.add_total_bytes(content_length);
        }
    }

    pub(super) fn record_chunk(&self, byte_count: u64) {
        self.transfer_progress.add_transferred_bytes(byte_count);
        self.control.downloaded(byte_count);
        let emitted_chunk_bytes = self
            .emitted_chunk_bytes
            .fetch_add(byte_count, AtomicOrdering::Relaxed)
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
            self.emitted_chunk_bytes.store(0, AtomicOrdering::Relaxed);
        }
    }

    pub(super) fn finish(&self) {
        emit_progress(
            self.control.progress(),
            GutchProgressEvent {
                feedback: None,
                operation: GutchOperation::Fetch,
                phase: self.phase,
                message: "Downloaded snapshot artifacts".to_string(),
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
            feedback: None,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    #[test]
    fn transfer_stats_report_bytes_and_average_rate() {
        let progress = FetchTransferProgress {
            started_at: Instant::now() - Duration::from_secs(1),
            ..Default::default()
        };
        progress.add_total_bytes(8 * 1024 * 1024);
        progress.add_transferred_bytes(4 * 1024 * 1024);
        progress.finish();

        let stats = progress.stats(TransferConcurrencyPolicy::fixed(4));

        assert_eq!(stats.transferred_bytes, 4 * 1024 * 1024);
        assert_eq!(stats.total_bytes, 8 * 1024 * 1024);
        assert!(stats.elapsed_seconds >= 0.0);
        assert!(stats.mib_per_second > 0.0);
    }
}

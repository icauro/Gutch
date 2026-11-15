//! Shared transfer counters and live feedback.
use super::types::{TransferConcurrencyPolicy, TransferStats};
use crate::progress::{
    OperationControl, OperationKind, ProgressEvent, ProgressPhase, emit_progress,
};
use std::cell::Cell;
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
    fn add_total_bytes(&self, bytes: u64) {
        self.total_bytes.fetch_add(bytes, AtomicOrdering::Relaxed);
    }

    fn add_transferred_bytes(&self, bytes: u64) {
        self.transferred_bytes
            .fetch_add(bytes, AtomicOrdering::Relaxed);
    }

    /// Withdraws the bytes a failed attempt counted, so a retry counts them once.
    fn remove_attempt_bytes(&self, total_bytes: u64, transferred_bytes: u64) {
        self.total_bytes
            .fetch_sub(total_bytes, AtomicOrdering::Relaxed);
        self.transferred_bytes
            .fetch_sub(transferred_bytes, AtomicOrdering::Relaxed);
    }

    pub(crate) fn mark_artifact_completed(&self) {
        self.completed_artifact_count
            .fetch_add(1, AtomicOrdering::Relaxed);
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
        if let Ok(mut finished_at) = self.finished_at.lock()
            && finished_at.is_none()
        {
            *finished_at = Some(Instant::now());
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

/// Reports one download attempt. Dropping it unfinished withdraws the
/// attempt's bytes from the shared totals.
pub(super) struct TransferProgressReporter<'a> {
    control: &'a OperationControl,
    transfer_progress: &'a Arc<FetchTransferProgress>,
    phase: ProgressPhase,
    message: &'a str,
    counters: TransferPhaseCounters,
    detail: Option<&'a str>,
    unreported_bytes: Cell<u64>,
    attempt_total_bytes: Cell<u64>,
    attempt_transferred_bytes: Cell<u64>,
    finished: Cell<bool>,
}

impl<'a> TransferProgressReporter<'a> {
    pub(super) fn new(
        control: &'a OperationControl,
        transfer_progress: &'a Arc<FetchTransferProgress>,
        phase: ProgressPhase,
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
            unreported_bytes: Cell::new(0),
            attempt_total_bytes: Cell::new(0),
            attempt_transferred_bytes: Cell::new(0),
            finished: Cell::new(false),
        }
    }

    pub(super) fn record_content_length(&self, content_length: Option<u64>) {
        if let Some(content_length) = content_length {
            self.transfer_progress.add_total_bytes(content_length);
            self.attempt_total_bytes
                .set(self.attempt_total_bytes.get() + content_length);
        }
    }

    pub(super) fn record_chunk(&self, byte_count: u64) {
        self.transfer_progress.add_transferred_bytes(byte_count);
        self.attempt_transferred_bytes
            .set(self.attempt_transferred_bytes.get() + byte_count);
        self.control.downloaded(byte_count);
        let unreported_bytes = self.unreported_bytes.get() + byte_count;
        if unreported_bytes >= DOWNLOAD_PROGRESS_EMIT_BYTES {
            self.emit(self.counters.current);
            self.unreported_bytes.set(0);
        } else {
            self.unreported_bytes.set(unreported_bytes);
        }
    }

    /// Keeps the attempt's bytes and reports this item as complete.
    pub(super) fn finish(self) {
        self.finished.set(true);
        self.emit(
            self.counters
                .total
                .min(self.counters.current.saturating_add(1)),
        );
    }

    fn emit(&self, current: usize) {
        emit_progress(
            self.control.progress(),
            ProgressEvent {
                transferred_bytes: self.transfer_progress.transferred_bytes(),
                total_bytes: self.transfer_progress.total_bytes(),
                detail: self.detail.map(str::to_string),
                ..ProgressEvent::new(
                    OperationKind::Fetch,
                    self.phase,
                    self.message,
                    current,
                    self.counters.total.max(1),
                )
            },
        );
    }
}

impl Drop for TransferProgressReporter<'_> {
    fn drop(&mut self) {
        if !self.finished.get() {
            self.transfer_progress.remove_attempt_bytes(
                self.attempt_total_bytes.get(),
                self.attempt_transferred_bytes.get(),
            );
        }
    }
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
        assert!(stats.elapsed_seconds >= 1.0);
        assert!(stats.mib_per_second > 0.0 && stats.mib_per_second <= 4.0);
    }

    #[test]
    fn failed_attempts_do_not_count_toward_transfer_bytes() {
        let control = OperationControl::default();
        let progress = Arc::new(FetchTransferProgress::default());
        let counters = TransferPhaseCounters {
            current: 0,
            total: 1,
        };
        let attempt = || {
            let reporter = TransferProgressReporter::new(
                &control,
                &progress,
                ProgressPhase::Artifact,
                "Downloading",
                counters,
                None,
            );
            reporter.record_content_length(Some(10));
            reporter.record_chunk(6);
            reporter
        };
        drop(attempt());
        assert_eq!(
            (progress.total_bytes(), progress.transferred_bytes()),
            (0, 0)
        );
        attempt().finish();
        assert_eq!(
            (progress.total_bytes(), progress.transferred_bytes()),
            (10, 6)
        );
    }
}

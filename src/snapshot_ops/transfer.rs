use std::cmp::Ordering;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering as AtomicOrdering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::types::{
    BundleDownload, SnapshotFetchRequest, TransferConcurrencyPolicy, TransferStats,
};
use crate::manifest::gutch_repo::GutchRepoManifest;
use crate::progress::{
    GutchCancellationHook, GutchOperation, GutchOperationControl, GutchProgressEvent,
    GutchProgressPhase, GutchProgressSink, emit_progress,
};
use crate::{Error, Result};

const DOWNLOAD_PROGRESS_EMIT_BYTES: u64 = 256 * 1024;
const MAX_TRANSFER_CONCURRENCY: usize = 16;
const MAX_ARTIFACT_DOWNLOAD_ATTEMPTS: usize = 3;
const ADAPTIVE_SAMPLE_INTERVAL: Duration = Duration::from_secs(2);
const ADAPTIVE_MIN_IMPROVEMENT: f64 = 1.05;
const ADAPTIVE_MAX_REGRESSION: f64 = 0.90;
const ADAPTIVE_STABLE_WINDOWS: usize = 2;
pub(super) const MAX_SNAPSHOT_METADATA_BYTES: u64 = 16 * 1024 * 1024;
pub(super) const MAX_SNAPSHOT_BLUEPRINT_BYTES: u64 = 256 * 1024 * 1024;
pub(super) const MAX_CHUNK_INDEX_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug)]
pub(super) struct FetchTransferProgress {
    started_at: Instant,
    finished_at: Mutex<Option<Instant>>,
    transferred_bytes: AtomicU64,
    total_bytes: AtomicU64,
    completed_bundle_count: AtomicUsize,
    active_download_count: AtomicUsize,
    peak_download_count: AtomicUsize,
    retry_count: AtomicUsize,
    adjustment_count: AtomicUsize,
}

impl Default for FetchTransferProgress {
    fn default() -> Self {
        Self {
            started_at: Instant::now(),
            finished_at: Mutex::new(None),
            transferred_bytes: AtomicU64::default(),
            total_bytes: AtomicU64::default(),
            completed_bundle_count: AtomicUsize::default(),
            active_download_count: AtomicUsize::default(),
            peak_download_count: AtomicUsize::default(),
            retry_count: AtomicUsize::default(),
            adjustment_count: AtomicUsize::default(),
        }
    }
}

impl FetchTransferProgress {
    pub(super) fn add_total_bytes(&self, bytes: u64) -> u64 {
        self.total_bytes.fetch_add(bytes, AtomicOrdering::Relaxed) + bytes
    }

    pub(super) fn add_transferred_bytes(&self, bytes: u64) -> u64 {
        self.transferred_bytes
            .fetch_add(bytes, AtomicOrdering::Relaxed)
            + bytes
    }

    pub(super) fn mark_bundle_completed(&self) -> usize {
        self.completed_bundle_count
            .fetch_add(1, AtomicOrdering::Relaxed)
            + 1
    }

    pub(super) fn completed_bundle_count(&self) -> usize {
        self.completed_bundle_count.load(AtomicOrdering::Relaxed)
    }

    pub(super) fn transferred_bytes(&self) -> u64 {
        self.transferred_bytes.load(AtomicOrdering::Relaxed)
    }

    pub(super) fn total_bytes(&self) -> u64 {
        self.total_bytes.load(AtomicOrdering::Relaxed)
    }

    pub(super) fn finish(&self) {
        if let Ok(mut finished_at) = self.finished_at.lock() {
            if finished_at.is_none() {
                *finished_at = Some(Instant::now());
            }
        }
    }

    fn download_started(&self) {
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

    fn download_finished(&self) {
        self.active_download_count
            .fetch_sub(1, AtomicOrdering::AcqRel);
    }

    fn retry_recorded(&self) {
        self.retry_count.fetch_add(1, AtomicOrdering::Relaxed);
    }

    pub(super) fn adjustment_recorded(&self) {
        self.adjustment_count.fetch_add(1, AtomicOrdering::Relaxed);
    }

    pub(super) fn retry_count(&self) -> usize {
        self.retry_count.load(AtomicOrdering::Acquire)
    }

    pub(super) fn stats(&self, policy: TransferConcurrencyPolicy) -> TransferStats {
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
    pub artifact: crate::ArtifactRef,
    pub bundle_id: String,
    pub url: String,
    pub bundle_path: PathBuf,
    pub ordinal: usize,
    pub total_missing_bundle_count: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct ArtifactPriority {
    pub ready_file_bytes: u64,
    pub near_ready_score: u64,
    pub dependent_file_count: usize,
}

impl ArtifactPriority {
    fn compare(self, other: Self) -> Ordering {
        self.ready_file_bytes
            .cmp(&other.ready_file_bytes)
            .then(self.near_ready_score.cmp(&other.near_ready_score))
            .then(self.dependent_file_count.cmp(&other.dependent_file_count))
    }
}

pub(super) enum ArtifactDownloadEvent {
    Ready(crate::ArtifactRef),
    Failed(Error),
    WorkerDone,
}

struct ArtifactDownloadQueueState {
    pending: Vec<PendingBundleDownload>,
    priorities: HashMap<crate::ArtifactRef, ArtifactPriority>,
    active_worker_count: usize,
    stopped: bool,
}

struct ArtifactDownloadQueueShared {
    state: Mutex<ArtifactDownloadQueueState>,
    wake: Condvar,
    target_worker_count: Arc<AtomicUsize>,
    cancelled: Arc<AtomicBool>,
}

#[derive(Debug)]
struct SchedulerCancellationHook {
    parent: GutchOperationControl,
    cancelled: Arc<AtomicBool>,
}

impl GutchCancellationHook for SchedulerCancellationHook {
    fn is_cancelled(&self) -> bool {
        self.cancelled.load(AtomicOrdering::Acquire) || self.parent.is_cancelled()
    }
}

pub(super) struct ArtifactDownloadScheduler {
    shared: Arc<ArtifactDownloadQueueShared>,
    events: mpsc::Receiver<ArtifactDownloadEvent>,
    worker_handles: Vec<JoinHandle<()>>,
    controller_stop: Arc<AtomicBool>,
    controller_handle: Option<JoinHandle<()>>,
}

impl ArtifactDownloadScheduler {
    pub(super) fn new(
        pending: &[PendingBundleDownload],
        priorities: HashMap<crate::ArtifactRef, ArtifactPriority>,
        policy: TransferConcurrencyPolicy,
        download_client: &reqwest::blocking::Client,
        control: &GutchOperationControl,
        transfer_progress: Arc<FetchTransferProgress>,
    ) -> Result<Self> {
        let (initial_workers, maximum_workers) =
            resolve_transfer_worker_limits(policy, pending.len())?;
        let target_worker_count = Arc::new(AtomicUsize::new(initial_workers));
        let cancelled = Arc::new(AtomicBool::new(false));
        let shared = Arc::new(ArtifactDownloadQueueShared {
            state: Mutex::new(ArtifactDownloadQueueState {
                pending: pending.to_vec(),
                priorities,
                active_worker_count: 0,
                stopped: false,
            }),
            wake: Condvar::new(),
            target_worker_count: Arc::clone(&target_worker_count),
            cancelled: Arc::clone(&cancelled),
        });
        let (event_tx, events) = mpsc::channel();
        let worker_control =
            control
                .clone()
                .with_cancellation(Arc::new(SchedulerCancellationHook {
                    parent: control.clone(),
                    cancelled: Arc::clone(&cancelled),
                }));
        let mut worker_handles = Vec::with_capacity(maximum_workers);
        for _ in 0..maximum_workers {
            let shared = Arc::clone(&shared);
            let event_tx = event_tx.clone();
            let download_client = download_client.clone();
            let control = worker_control.clone();
            let transfer_progress = Arc::clone(&transfer_progress);
            worker_handles.push(thread::spawn(move || {
                artifact_download_worker(
                    shared,
                    event_tx,
                    download_client,
                    control,
                    transfer_progress,
                );
            }));
        }
        drop(event_tx);

        let controller_stop = Arc::new(AtomicBool::new(false));
        let controller_handle = if matches!(policy, TransferConcurrencyPolicy::Adaptive { .. }) {
            let controller_stop = Arc::clone(&controller_stop);
            let shared = Arc::clone(&shared);
            let transfer_progress = Arc::clone(&transfer_progress);
            let controller_stop_for_thread = Arc::clone(&controller_stop);
            let control = control.clone();
            Some(thread::spawn(move || {
                run_adaptive_transfer_controller(
                    policy,
                    shared,
                    transfer_progress,
                    controller_stop_for_thread,
                    control,
                );
            }))
        } else {
            None
        };

        Ok(Self {
            shared,
            events,
            worker_handles,
            controller_stop,
            controller_handle,
        })
    }

    pub(super) fn recv(&self) -> std::result::Result<ArtifactDownloadEvent, mpsc::RecvError> {
        self.events.recv()
    }

    pub(super) fn try_recv(
        &self,
    ) -> std::result::Result<ArtifactDownloadEvent, mpsc::TryRecvError> {
        self.events.try_recv()
    }

    pub(super) fn cancel(&self) {
        self.shared.cancelled.store(true, AtomicOrdering::Release);
        if let Ok(mut state) = self.shared.state.lock() {
            state.stopped = true;
            state.pending.clear();
            self.shared.wake.notify_all();
        }
    }

    pub(super) fn join(mut self) -> Result<()> {
        self.controller_stop.store(true, AtomicOrdering::Release);
        self.shared.wake.notify_all();
        if let Some(handle) = self.controller_handle.take() {
            handle
                .join()
                .map_err(|_| Error::reconstruct("adaptive transfer controller panicked"))?;
        }
        for handle in self.worker_handles {
            handle
                .join()
                .map_err(|_| Error::reconstruct("artifact download worker panicked"))?;
        }
        Ok(())
    }

    pub(super) fn worker_count(&self) -> usize {
        self.worker_handles.len()
    }
}

fn resolve_transfer_worker_limits(
    policy: TransferConcurrencyPolicy,
    pending_count: usize,
) -> Result<(usize, usize)> {
    let (initial, maximum) = match policy {
        TransferConcurrencyPolicy::Fixed(concurrency) => (concurrency, concurrency),
        TransferConcurrencyPolicy::Adaptive { initial, maximum } => (initial, maximum),
    };
    if initial == 0 || maximum == 0 || initial > maximum || maximum > MAX_TRANSFER_CONCURRENCY {
        return Err(Error::manifest(format!(
            "transfer concurrency must be between 1 and {MAX_TRANSFER_CONCURRENCY}, with initial no greater than maximum"
        )));
    }
    if pending_count == 0 {
        return Ok((0, 0));
    }
    Ok((initial.min(pending_count), maximum.min(pending_count)))
}

fn select_next_artifact(state: &ArtifactDownloadQueueState) -> Option<usize> {
    state
        .pending
        .iter()
        .enumerate()
        .max_by(|(_, left), (_, right)| {
            let left_priority = state
                .priorities
                .get(&left.artifact)
                .copied()
                .unwrap_or_default();
            let right_priority = state
                .priorities
                .get(&right.artifact)
                .copied()
                .unwrap_or_default();
            left_priority
                .compare(right_priority)
                .then_with(|| right.bundle_id.cmp(&left.bundle_id))
        })
        .map(|(index, _)| index)
}

fn next_artifact(shared: &ArtifactDownloadQueueShared) -> Option<PendingBundleDownload> {
    let mut state = shared.state.lock().ok()?;
    loop {
        if state.stopped || shared.cancelled.load(AtomicOrdering::Acquire) {
            return None;
        }
        let target_worker_count = shared.target_worker_count.load(AtomicOrdering::Acquire);
        if state.active_worker_count < target_worker_count {
            let index = select_next_artifact(&state)?;
            state.active_worker_count += 1;
            return Some(state.pending.swap_remove(index));
        }
        state = shared.wake.wait(state).ok()?;
    }
}

fn artifact_download_worker(
    shared: Arc<ArtifactDownloadQueueShared>,
    event_tx: mpsc::Sender<ArtifactDownloadEvent>,
    download_client: reqwest::blocking::Client,
    control: GutchOperationControl,
    transfer_progress: Arc<FetchTransferProgress>,
) {
    loop {
        let Some(bundle_download) = next_artifact(&shared) else {
            let _ = event_tx.send(ArtifactDownloadEvent::WorkerDone);
            return;
        };
        let _artifact_span = control.profile_span_with_args(
            "gutch.fetch.artifact",
            std::collections::BTreeMap::from([
                (
                    "ordinal".to_string(),
                    serde_json::json!(bundle_download.ordinal),
                ),
                (
                    "total".to_string(),
                    serde_json::json!(bundle_download.total_missing_bundle_count),
                ),
            ]),
        );
        let result = download_pending_bundle(
            &download_client,
            &bundle_download,
            &control,
            &transfer_progress,
        );
        if let Ok(mut state) = shared.state.lock() {
            state.active_worker_count = state.active_worker_count.saturating_sub(1);
            if result.is_err() {
                state.stopped = true;
                state.pending.clear();
            }
            shared.wake.notify_all();
        }
        match result {
            Ok(_) => {
                if event_tx
                    .send(ArtifactDownloadEvent::Ready(bundle_download.artifact))
                    .is_err()
                {
                    return;
                }
            }
            Err(error) => {
                shared.cancelled.store(true, AtomicOrdering::Release);
                let _ = event_tx.send(ArtifactDownloadEvent::Failed(error));
                return;
            }
        }
    }
}

struct AdaptiveTransferState {
    target: usize,
    maximum: usize,
    previous_sample_at: Instant,
    previous_bytes: u64,
    previous_rate: Option<f64>,
    stable_windows: usize,
    last_retry_count: usize,
}

impl AdaptiveTransferState {
    fn new(initial: usize, maximum: usize) -> Self {
        Self {
            target: initial,
            maximum,
            previous_sample_at: Instant::now(),
            previous_bytes: 0,
            previous_rate: None,
            stable_windows: 0,
            last_retry_count: 0,
        }
    }

    fn observe(
        &mut self,
        now: Instant,
        transferred_bytes: u64,
        retry_count: usize,
    ) -> Option<usize> {
        let elapsed = now.saturating_duration_since(self.previous_sample_at);
        if elapsed < ADAPTIVE_SAMPLE_INTERVAL {
            return None;
        }
        let rate = transferred_bytes.saturating_sub(self.previous_bytes) as f64
            / elapsed.as_secs_f64().max(f64::EPSILON);
        let had_new_retry = retry_count > self.last_retry_count;
        let previous_rate = self.previous_rate;
        self.previous_sample_at = now;
        self.previous_bytes = transferred_bytes;
        self.previous_rate = Some(rate);
        self.last_retry_count = retry_count;

        if had_new_retry {
            self.stable_windows = 0;
            return self.reduce_target();
        }
        let Some(previous_rate) = previous_rate else {
            return None;
        };
        if rate < previous_rate * ADAPTIVE_MAX_REGRESSION {
            self.stable_windows = 0;
            return self.reduce_target();
        }
        if rate >= previous_rate * ADAPTIVE_MIN_IMPROVEMENT {
            self.stable_windows += 1;
            if self.stable_windows >= ADAPTIVE_STABLE_WINDOWS {
                self.stable_windows = 0;
                return self.increase_target();
            }
        } else {
            self.stable_windows = 0;
        }
        None
    }

    fn increase_target(&mut self) -> Option<usize> {
        if self.target >= self.maximum {
            return None;
        }
        self.target += 1;
        Some(self.target)
    }

    fn reduce_target(&mut self) -> Option<usize> {
        if self.target <= 1 {
            return None;
        }
        self.target -= 1;
        Some(self.target)
    }
}

fn run_adaptive_transfer_controller(
    policy: TransferConcurrencyPolicy,
    shared: Arc<ArtifactDownloadQueueShared>,
    transfer_progress: Arc<FetchTransferProgress>,
    controller_stop: Arc<AtomicBool>,
    control: GutchOperationControl,
) {
    let TransferConcurrencyPolicy::Adaptive { initial, maximum } = policy else {
        return;
    };
    let mut controller = AdaptiveTransferState::new(initial, maximum);
    while !controller_stop.load(AtomicOrdering::Acquire) {
        for _ in 0..20 {
            if controller_stop.load(AtomicOrdering::Acquire) {
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }
        if controller_stop.load(AtomicOrdering::Acquire) {
            return;
        }
        let Some(new_target) = controller.observe(
            Instant::now(),
            transfer_progress.transferred_bytes(),
            transfer_progress.retry_count(),
        ) else {
            continue;
        };
        shared
            .target_worker_count
            .store(new_target, AtomicOrdering::Release);
        control.profile_event_with_args(
            "gutch.fetch.transfer_concurrency",
            std::collections::BTreeMap::from([(
                "target_workers".to_string(),
                serde_json::json!(new_target),
            )]),
        );
        transfer_progress.adjustment_recorded();
        shared.wake.notify_all();
    }
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

    fn finish(&self) {
        emit_progress(
            self.control.progress(),
            GutchProgressEvent {
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

pub(crate) fn snapshot_manifest_from_download(
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
    let _artifact_span = control.profile_span_with_args(
        "gutch.fetch.artifact.transfer",
        std::collections::BTreeMap::from([
            (
                "ordinal".to_string(),
                serde_json::json!(bundle_download.ordinal),
            ),
            (
                "total".to_string(),
                serde_json::json!(bundle_download.total_missing_bundle_count),
            ),
        ]),
    );
    if let Some(parent) = bundle_download.bundle_path.parent() {
        crate::helpers::create_dir_all(parent)?;
    }
    transfer_progress.download_started();
    let result = (|| -> Result<()> {
        let message = format!(
            "Downloading artifacts ({}/{})",
            bundle_download.ordinal + 1,
            bundle_download.total_missing_bundle_count.max(1)
        );
        for attempt in 1..=MAX_ARTIFACT_DOWNLOAD_ATTEMPTS {
            control.checkpoint()?;
            let attempt_result = TransferDownloadRequest {
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
            .download_to_path(&bundle_download.bundle_path);
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
    Ok(transfer_progress.mark_bundle_completed())
}

fn retry_delay(attempt: usize) -> Duration {
    Duration::from_millis(250_u64.saturating_mul(1_u64 << attempt.saturating_sub(1)))
}

fn sleep_with_checkpoint(control: &GutchOperationControl, delay: Duration) -> Result<()> {
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

#[cfg(test)]
mod tests {
    use super::{
        ADAPTIVE_SAMPLE_INTERVAL, AdaptiveTransferState, ArtifactPriority, FetchTransferProgress,
        resolve_transfer_worker_limits,
    };
    use crate::TransferConcurrencyPolicy;
    use std::time::Duration;

    #[test]
    fn transfer_worker_limits_clamp_to_pending_artifacts() {
        assert_eq!(
            resolve_transfer_worker_limits(TransferConcurrencyPolicy::fixed(4), 2).unwrap(),
            (2, 2)
        );
        assert_eq!(
            resolve_transfer_worker_limits(TransferConcurrencyPolicy::adaptive(4, 16), 32).unwrap(),
            (4, 16)
        );
        assert!(resolve_transfer_worker_limits(TransferConcurrencyPolicy::fixed(0), 2).is_err());
        assert!(
            resolve_transfer_worker_limits(TransferConcurrencyPolicy::adaptive(8, 4), 2).is_err()
        );
    }

    #[test]
    fn adaptive_transfer_state_ramps_up_after_stable_improvement() {
        let mut state = AdaptiveTransferState::new(4, 8);
        let start = state.previous_sample_at;
        assert_eq!(
            state.observe(start + ADAPTIVE_SAMPLE_INTERVAL, 100, 0),
            None
        );
        assert_eq!(state.observe(start + Duration::from_secs(4), 220, 0), None);
        assert_eq!(
            state.observe(start + Duration::from_secs(6), 360, 0),
            Some(5)
        );
    }

    #[test]
    fn adaptive_transfer_state_reduces_on_regression() {
        let mut state = AdaptiveTransferState::new(4, 8);
        let start = state.previous_sample_at;
        state.observe(start + ADAPTIVE_SAMPLE_INTERVAL, 100, 0);
        assert_eq!(
            state.observe(start + Duration::from_secs(4), 140, 0),
            Some(3)
        );
        assert_eq!(
            state.observe(start + Duration::from_secs(6), 150, 0),
            Some(2)
        );
    }

    #[test]
    fn artifact_priority_prefers_files_that_can_become_ready() {
        let ready = ArtifactPriority {
            ready_file_bytes: 4,
            near_ready_score: 0,
            dependent_file_count: 1,
        };
        let near_ready = ArtifactPriority {
            ready_file_bytes: 0,
            near_ready_score: 100,
            dependent_file_count: 4,
        };
        assert!(ready.compare(near_ready).is_gt());
    }

    #[test]
    fn transfer_stats_report_bytes_and_average_rate() {
        let progress = FetchTransferProgress::default();
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

//! Artifact download queue: priorities, scheduling, adaptive concurrency and readiness events.

use super::adaptive::{ADAPTIVE_SAMPLE_INTERVAL, AdaptiveTransferState, TransferObservation};
use super::artifact_download::download_pending_artifact;
use super::progress::FetchTransferProgress;
use crate::artifact::seal::SealKeys;
use crate::profiling::Arguments;
use crate::profiling::trace_args;
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Condvar;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering as AtomicOrdering;
use std::sync::mpsc;
use std::thread;
use std::thread::JoinHandle;
use std::time::Duration;
use std::time::Instant;

use crate::Error;
use crate::Result;
use crate::artifact::reference::ArtifactRef;
use crate::progress::CancellationHook;
use crate::progress::OperationControl;
use crate::transfer::http::DownloadClient;
use crate::transfer::types::TransferConcurrencyPolicy;

/// How often the adaptive controller checks for a stop request between samples.
const CONTROLLER_STOP_POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone)]
pub(crate) struct PendingArtifactDownload {
    pub artifact: ArtifactRef,
    pub artifact_id: String,
    pub url: String,
    pub artifact_path: PathBuf,
    pub ordinal: usize,
    pub missing_artifact_count: usize,
    /// The repository's keys when it is encrypted: the artifact is sealed.
    pub keys: Option<Arc<SealKeys>>,
}

/// Download order for the max-heap, compared field by field: artifacts that
/// can finish the most file bytes now, then those closest to making more files
/// ready, then the broadest reuse. `near_ready_score` sums file bytes divided by
/// missing artifacts for files needing more than one artifact.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ArtifactPriority {
    pub ready_file_bytes: u64,
    pub near_ready_score: u64,
    pub dependent_file_count: usize,
}

pub(crate) enum ArtifactDownloadEvent {
    Ready(ArtifactRef),
    Failed(Error),
    WorkerDone,
}

struct ArtifactDownloadQueueState {
    pending: BinaryHeap<QueuedArtifact>,
    active_worker_count: usize,
    stopped: bool,
}

struct QueuedArtifact {
    pending: PendingArtifactDownload,
    priority: ArtifactPriority,
}

impl PartialEq for QueuedArtifact {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for QueuedArtifact {}

impl Ord for QueuedArtifact {
    fn cmp(&self, other: &Self) -> Ordering {
        self.priority
            .cmp(&other.priority)
            .then_with(|| self.pending.artifact_id.cmp(&other.pending.artifact_id))
    }
}

impl PartialOrd for QueuedArtifact {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

struct ArtifactDownloadQueueShared {
    state: Mutex<ArtifactDownloadQueueState>,
    wake: Condvar,
    target_worker_count: Arc<AtomicUsize>,
    cancelled: Arc<AtomicBool>,
}

#[derive(Debug)]
struct SchedulerCancellationHook {
    parent: OperationControl,
    cancelled: Arc<AtomicBool>,
}

impl CancellationHook for SchedulerCancellationHook {
    fn is_cancelled(&self) -> bool {
        self.cancelled.load(AtomicOrdering::Acquire) || self.parent.is_cancelled()
    }
}

pub(crate) struct ArtifactDownloadScheduler {
    shared: Arc<ArtifactDownloadQueueShared>,
    events: mpsc::Receiver<ArtifactDownloadEvent>,
    worker_handles: Vec<JoinHandle<()>>,
    controller_stop: Arc<AtomicBool>,
    controller_handle: Option<JoinHandle<()>>,
}

impl ArtifactDownloadScheduler {
    pub(crate) fn new(
        pending: &[PendingArtifactDownload],
        priorities: HashMap<ArtifactRef, ArtifactPriority>,
        policy: TransferConcurrencyPolicy,
        download_client: &DownloadClient,
        control: &OperationControl,
        transfer_progress: Arc<FetchTransferProgress>,
    ) -> Result<Self> {
        let (initial_workers, maximum_workers) =
            resolve_transfer_worker_limits(policy, pending.len())?;
        let target_worker_count = Arc::new(AtomicUsize::new(initial_workers));
        let cancelled = Arc::new(AtomicBool::new(false));
        let shared = Arc::new(ArtifactDownloadQueueShared {
            state: Mutex::new(ArtifactDownloadQueueState {
                pending: pending
                    .iter()
                    .cloned()
                    .map(|pending| {
                        let priority = priorities
                            .get(&pending.artifact)
                            .copied()
                            .unwrap_or_default();
                        QueuedArtifact { pending, priority }
                    })
                    .collect(),
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

    pub(crate) fn recv(&self) -> std::result::Result<ArtifactDownloadEvent, mpsc::RecvError> {
        self.events.recv()
    }

    pub(crate) fn try_recv(
        &self,
    ) -> std::result::Result<ArtifactDownloadEvent, mpsc::TryRecvError> {
        self.events.try_recv()
    }

    pub(crate) fn cancel(&self) {
        self.shared.cancelled.store(true, AtomicOrdering::Release);
        if let Ok(mut state) = self.shared.state.lock() {
            state.stopped = true;
            state.pending.clear();
            self.shared.wake.notify_all();
        }
    }

    pub(crate) fn join(mut self) -> Result<()> {
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

    pub(crate) fn worker_count(&self) -> usize {
        self.worker_handles.len()
    }
}

fn resolve_transfer_worker_limits(
    policy: TransferConcurrencyPolicy,
    pending_count: usize,
) -> Result<(usize, usize)> {
    policy.validate()?;
    Ok((
        policy.initial().min(pending_count),
        policy.maximum().min(pending_count),
    ))
}

fn next_artifact(shared: &ArtifactDownloadQueueShared) -> Option<PendingArtifactDownload> {
    let mut state = shared.state.lock().ok()?;
    loop {
        if state.stopped || shared.cancelled.load(AtomicOrdering::Acquire) {
            return None;
        }
        let target_worker_count = shared.target_worker_count.load(AtomicOrdering::Acquire);
        if state.active_worker_count < target_worker_count {
            let artifact = state.pending.pop()?;
            state.active_worker_count += 1;
            return Some(artifact.pending);
        }
        state = shared.wake.wait(state).ok()?;
    }
}

fn artifact_download_worker(
    shared: Arc<ArtifactDownloadQueueShared>,
    event_tx: mpsc::Sender<ArtifactDownloadEvent>,
    download_client: DownloadClient,
    control: OperationControl,
    transfer_progress: Arc<FetchTransferProgress>,
) {
    loop {
        let Some(artifact_download) = next_artifact(&shared) else {
            let _ = event_tx.send(ArtifactDownloadEvent::WorkerDone);
            return;
        };
        let _artifact_span =
            control.profile_span_with_args("fetch.artifact", || artifact_download.trace_args());
        let result = download_pending_artifact(
            &download_client,
            &artifact_download,
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
            Ok(()) => {
                if event_tx
                    .send(ArtifactDownloadEvent::Ready(artifact_download.artifact))
                    .is_err()
                {
                    return;
                }
            }
            Err(error) => {
                // Queue the real failure before cancelling siblings, so their
                // resulting `Cancelled` events can only arrive after it.
                let _ = event_tx.send(ArtifactDownloadEvent::Failed(error));
                shared.cancelled.store(true, AtomicOrdering::Release);
                return;
            }
        }
    }
}

fn run_adaptive_transfer_controller(
    policy: TransferConcurrencyPolicy,
    shared: Arc<ArtifactDownloadQueueShared>,
    transfer_progress: Arc<FetchTransferProgress>,
    controller_stop: Arc<AtomicBool>,
    control: OperationControl,
) {
    let TransferConcurrencyPolicy::Adaptive { initial, maximum } = policy else {
        return;
    };
    let mut controller = AdaptiveTransferState::new(initial, maximum);
    // Metadata may already have been fetched through these counters. Only bytes
    // arriving during the controller's observation belong in its first rate.
    controller.previous_bytes = transfer_progress.transferred_bytes();
    loop {
        let sample_at = Instant::now() + ADAPTIVE_SAMPLE_INTERVAL;
        loop {
            if controller_stop.load(AtomicOrdering::Acquire) {
                return;
            }
            let remaining = sample_at.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            thread::sleep(remaining.min(CONTROLLER_STOP_POLL_INTERVAL));
        }
        let (has_queued_work, active_workers) = shared
            .state
            .lock()
            .map(|state| {
                (
                    !state.stopped && !state.pending.is_empty(),
                    state.active_worker_count,
                )
            })
            .unwrap_or((false, 0));
        let Some(new_target) = controller.observe(TransferObservation {
            now: Instant::now(),
            transferred_bytes: transfer_progress.transferred_bytes(),
            retry_count: transfer_progress.retry_count(),
            downstream_backpressure: transfer_progress.downstream_backpressure(),
            has_queued_work,
            active_workers,
        }) else {
            continue;
        };
        shared
            .target_worker_count
            .store(new_target, AtomicOrdering::Release);
        control.profile_event_with_args(
            "transfer.concurrency",
            || trace_args!("target_workers" => new_target),
        );
        transfer_progress.adjustment_recorded();
        shared.wake.notify_all();
    }
}

impl PendingArtifactDownload {
    pub(super) fn trace_args(&self) -> Arguments {
        trace_args!(
            "artifact_id" => self.artifact.id(),
            "ordinal" => self.ordinal,
            "total" => self.missing_artifact_count,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(ready > near_ready);
    }

    fn queued(artifact_id: &str, ready_file_bytes: u64) -> QueuedArtifact {
        QueuedArtifact {
            pending: PendingArtifactDownload {
                artifact: ArtifactRef::Bundle {
                    id: artifact_id.into(),
                },
                artifact_id: artifact_id.to_string(),
                url: String::new(),
                artifact_path: PathBuf::new(),
                ordinal: 0,
                missing_artifact_count: 1,
                keys: None,
            },
            priority: ArtifactPriority {
                ready_file_bytes,
                ..Default::default()
            },
        }
    }

    #[test]
    fn queue_pops_highest_priority_then_highest_id_and_eq_matches_ord() {
        let mut heap = BinaryHeap::from([queued("a", 1), queued("c", 1), queued("b", 9)]);
        let order = std::iter::from_fn(|| heap.pop().map(|item| item.pending.artifact_id))
            .collect::<Vec<_>>();
        assert_eq!(order, ["b", "c", "a"]);
        assert!(queued("a", 1) != queued("a", 2));
    }
}

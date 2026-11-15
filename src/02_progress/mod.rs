//! Operation control, cancellation, progress events, and bounded live feedback.

mod feedback;
pub use feedback::{LiveFeedback, Throughput};
use std::sync::Arc;

use serde::Serialize;

use crate::Error;
use crate::Result;

/// Identifies which gutch operation is emitting progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum OperationKind {
    SnapshotCapture,
    SnapshotDelete,
    Push,
    Pull,
    SnapshotSourcesCapture,
    Fetch,
    Verify,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgressPhase {
    Prepare,
    RepoMetadata,
    SnapshotMetadata,
    ChunkIndex,
    Artifact,
    Reconstruct,
    Json,
    Done,
    ScanSnapshots,
    LoadManifests,
    ScanArtifacts,
    DeleteOrphans,
    RemoveManifest,
    Scan,
    Planning,
    RemoveSnapshots,
    UploadRepoMetadata,
    UploadChunkIndex,
    UploadSnapshotMetadata,
    UploadSnapshotBlueprint,
    UploadBundle,
    Prune,
    Capture,
    Verify,
}

impl ProgressPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            ProgressPhase::Prepare => "prepare",
            ProgressPhase::RepoMetadata => "repo_metadata",
            ProgressPhase::SnapshotMetadata => "snapshot_metadata",
            ProgressPhase::ChunkIndex => "chunk_index",
            ProgressPhase::Artifact => "artifact",
            ProgressPhase::Reconstruct => "reconstruct",
            ProgressPhase::Json => "json",
            ProgressPhase::Done => "done",
            ProgressPhase::ScanSnapshots => "scan_snapshots",
            ProgressPhase::LoadManifests => "load_manifests",
            ProgressPhase::ScanArtifacts => "scan_artifacts",
            ProgressPhase::DeleteOrphans => "delete_orphans",
            ProgressPhase::RemoveManifest => "remove_manifest",
            ProgressPhase::Scan => "scan",
            ProgressPhase::Planning => "planning",
            ProgressPhase::RemoveSnapshots => "remove_snapshots",
            ProgressPhase::UploadRepoMetadata => "upload_repo_metadata",
            ProgressPhase::UploadChunkIndex => "upload_chunk_index",
            ProgressPhase::UploadSnapshotMetadata => "upload_snapshot_metadata",
            ProgressPhase::UploadSnapshotBlueprint => "upload_snapshot_blueprint",
            ProgressPhase::UploadBundle => "upload_bundle",
            ProgressPhase::Prune => "prune",
            ProgressPhase::Capture => "capture",
            ProgressPhase::Verify => "verify",
        }
    }
}

/// A single unified progress event for all gutch operations.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressEvent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feedback: Option<LiveFeedback>,
    /// Which operation is running.
    pub operation: OperationKind,
    /// Current phase within the operation (e.g. "scan", "capture", "upload", "apply").
    pub phase: ProgressPhase,
    /// Human-readable message for the current step.
    pub message: String,
    /// Current progress counter (files scanned, bundles built, snapshots synced, etc.).
    pub current: usize,
    /// Total expected count for this phase.
    pub total: usize,
    /// Bytes transferred so far (for network operations).
    #[serde(default)]
    pub transferred_bytes: u64,
    /// Total bytes expected (for network operations).
    #[serde(default)]
    pub total_bytes: u64,
    /// Optional extra detail (current file name, bundle id, etc.).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// True when the engine has emitted its final progress stage. Errors still return through Result.
    #[serde(default)]
    pub done: bool,
}

/// Trait for receiving progress events. Implemented by CLI (indicatif) and Tauri (event emitter).
pub trait ProgressSink: Send + Sync + std::fmt::Debug {
    fn emit(&self, event: ProgressEvent);
}

/// Read-only cancellation boundary implemented by the calling application.
///
/// Gutch deliberately does not own the mutable signal or depend on an async runtime. Long-running
/// engine code observes this hook only at consistency-safe checkpoints.
pub trait CancellationHook: Send + Sync + std::fmt::Debug {
    fn is_cancelled(&self) -> bool;
}

/// Cross-cutting controls propagated through one complete Gutch operation, including nested work.
#[derive(Clone, Debug, Default)]
pub struct OperationControl {
    progress: Option<Arc<dyn ProgressSink>>,
    cancellation: Option<Arc<dyn CancellationHook>>,
    profiler: Option<Arc<crate::profiling::Profiler>>,
    feedback: Option<Arc<feedback::LiveProgress>>,
}

impl OperationControl {
    pub fn with_progress(mut self, progress: Arc<dyn ProgressSink>) -> Self {
        let feedback = feedback::LiveProgress::new(progress);
        if let Some(profiler) = &self.profiler {
            feedback.with_profiler(profiler.clone());
        }
        self.progress = Some(feedback.clone());
        self.feedback = Some(feedback);
        self
    }

    pub(crate) fn output_stage(&self) -> feedback::OutputStage {
        feedback::OutputStage::new(self.feedback.clone())
    }
    pub(crate) fn output_finished(&self) {
        if let Some(feedback) = &self.feedback {
            feedback.finish(false);
        }
    }
    pub(crate) fn download_started(&self) {
        if let Some(f) = &self.feedback {
            f.start(true);
        }
    }
    pub(crate) fn download_finished(&self) {
        if let Some(f) = &self.feedback {
            f.finish(true);
        }
    }
    pub(crate) fn downloaded(&self, bytes: u64) {
        if let Some(f) = &self.feedback {
            f.record(true, bytes);
        }
    }
    pub(crate) fn written(&self, bytes: u64) {
        if let Some(f) = &self.feedback {
            f.record(false, bytes);
        }
    }
    pub fn with_cancellation(mut self, cancellation: Arc<dyn CancellationHook>) -> Self {
        self.cancellation = Some(cancellation);
        self
    }

    pub fn with_profiler(mut self, profiler: Arc<crate::profiling::Profiler>) -> Self {
        if let Some(feedback) = &self.feedback {
            feedback.with_profiler(profiler.clone());
        }
        self.profiler = Some(profiler);
        self
    }

    /// Replaces only the progress adapter while retaining the operation's cancellation hook.
    pub(crate) fn with_replaced_progress(&self, progress: Arc<dyn ProgressSink>) -> Self {
        Self {
            progress: Some(progress),
            cancellation: self.cancellation.clone(),
            profiler: self.profiler.clone(),
            feedback: self.feedback.clone(),
        }
    }

    pub(crate) fn profile_span(&self, name: &'static str) -> Option<crate::profiling::ProfileSpan> {
        self.profiler
            .as_ref()
            .and_then(|profiler| profiler.span(name))
    }

    pub(crate) fn profiler(&self) -> Option<&crate::profiling::Profiler> {
        self.profiler.as_deref()
    }

    pub(crate) fn profile_span_with_args(
        &self,
        name: &'static str,
        args: impl FnOnce() -> std::collections::BTreeMap<String, serde_json::Value>,
    ) -> Option<crate::profiling::ProfileSpan> {
        self.profiler
            .as_ref()
            .and_then(|profiler| profiler.span_with_args(name, args))
    }

    pub(crate) fn profile_event_with_args(
        &self,
        name: &'static str,
        args: impl FnOnce() -> std::collections::BTreeMap<String, serde_json::Value>,
    ) {
        if let Some(profiler) = &self.profiler {
            profiler.event_with_args(name, args);
        }
    }

    pub(crate) fn progress(&self) -> &Option<Arc<dyn ProgressSink>> {
        &self.progress
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancellation
            .as_ref()
            .is_some_and(|hook| hook.is_cancelled())
    }

    pub fn checkpoint(&self) -> Result<()> {
        if self.is_cancelled() {
            return Err(Error::Cancelled);
        }
        Ok(())
    }
}

/// Helper to emit if a sink is present.
pub(crate) fn emit_progress(sink: &Option<std::sync::Arc<dyn ProgressSink>>, event: ProgressEvent) {
    if let Some(s) = sink {
        s.emit(event);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::Ordering;

    use super::CancellationHook;
    use super::OperationControl;

    #[derive(Debug)]
    struct TestCancellation(AtomicBool);

    impl CancellationHook for TestCancellation {
        fn is_cancelled(&self) -> bool {
            self.0.load(Ordering::Acquire)
        }
    }

    #[test]
    fn operation_control_returns_typed_cancellation() {
        let cancellation = Arc::new(TestCancellation(AtomicBool::new(false)));
        let control = OperationControl::default().with_cancellation(cancellation.clone());

        assert!(control.checkpoint().is_ok());
        cancellation.0.store(true, Ordering::Release);
        assert!(matches!(control.checkpoint(), Err(crate::Error::Cancelled)));
    }
}

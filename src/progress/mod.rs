use std::sync::Arc;

use serde::Serialize;

use crate::{Error, Result};

/// Identifies which gutch operation is emitting progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum GutchOperation {
    SnapshotCapture,
    SnapshotDelete,
    SyncUp,
    Pull,
    SnapshotSourcesCapture,
    Fetch,
    Verify,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GutchProgressPhase {
    Prepare,
    RepoMetadata,
    SnapshotMetadata,
    ChunkIndex,
    Bundle,
    Reconstruct,
    Json,
    Done,
    ScanSnapshots,
    LoadManifests,
    ScanBundles,
    DeleteOrphans,
    RemoveManifest,
    Scan,
    Build,
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

impl GutchProgressPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            GutchProgressPhase::Prepare => "prepare",
            GutchProgressPhase::RepoMetadata => "repo_metadata",
            GutchProgressPhase::SnapshotMetadata => "snapshot_metadata",
            GutchProgressPhase::ChunkIndex => "chunk_index",
            GutchProgressPhase::Bundle => "bundle",
            GutchProgressPhase::Reconstruct => "reconstruct",
            GutchProgressPhase::Json => "json",
            GutchProgressPhase::Done => "done",
            GutchProgressPhase::ScanSnapshots => "scan_snapshots",
            GutchProgressPhase::LoadManifests => "load_manifests",
            GutchProgressPhase::ScanBundles => "scan_bundles",
            GutchProgressPhase::DeleteOrphans => "delete_orphans",
            GutchProgressPhase::RemoveManifest => "remove_manifest",
            GutchProgressPhase::Scan => "scan",
            GutchProgressPhase::Build => "build",
            GutchProgressPhase::Planning => "planning",
            GutchProgressPhase::RemoveSnapshots => "remove_snapshots",
            GutchProgressPhase::UploadRepoMetadata => "upload_repo_metadata",
            GutchProgressPhase::UploadChunkIndex => "upload_chunk_index",
            GutchProgressPhase::UploadSnapshotMetadata => "upload_snapshot_metadata",
            GutchProgressPhase::UploadSnapshotBlueprint => "upload_snapshot_blueprint",
            GutchProgressPhase::UploadBundle => "upload_bundle",
            GutchProgressPhase::Prune => "prune",
            GutchProgressPhase::Capture => "capture",
            GutchProgressPhase::Verify => "verify",
        }
    }
}

/// A single unified progress event for all gutch operations.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GutchProgressEvent {
    /// Which operation is running.
    pub operation: GutchOperation,
    /// Current phase within the operation (e.g. "scan", "build", "upload", "apply").
    pub phase: GutchProgressPhase,
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
pub trait GutchProgressSink: Send + Sync + std::fmt::Debug {
    fn emit(&self, event: GutchProgressEvent);
}

/// Read-only cancellation boundary implemented by the calling application.
///
/// Gutch deliberately does not own the mutable signal or depend on an async runtime. Long-running
/// engine code observes this hook only at consistency-safe checkpoints.
pub trait GutchCancellationHook: Send + Sync + std::fmt::Debug {
    fn is_cancelled(&self) -> bool;
}

/// Cross-cutting controls propagated through one complete Gutch operation, including nested work.
#[derive(Clone, Debug, Default)]
pub struct GutchOperationControl {
    progress: Option<Arc<dyn GutchProgressSink>>,
    cancellation: Option<Arc<dyn GutchCancellationHook>>,
    profiler: Option<Arc<crate::GutchProfiler>>,
}

impl GutchOperationControl {
    pub fn with_progress(mut self, progress: Arc<dyn GutchProgressSink>) -> Self {
        self.progress = Some(progress);
        self
    }

    pub fn with_cancellation(mut self, cancellation: Arc<dyn GutchCancellationHook>) -> Self {
        self.cancellation = Some(cancellation);
        self
    }

    pub fn with_profiler(mut self, profiler: Arc<crate::GutchProfiler>) -> Self {
        self.profiler = Some(profiler);
        self
    }

    /// Replaces only the progress adapter while retaining the operation's cancellation hook.
    pub(crate) fn with_replaced_progress(&self, progress: Arc<dyn GutchProgressSink>) -> Self {
        Self {
            progress: Some(progress),
            cancellation: self.cancellation.clone(),
            profiler: self.profiler.clone(),
        }
    }

    pub(crate) fn profile_span(&self, name: impl Into<String>) -> Option<crate::GutchProfileSpan> {
        self.profiler.as_ref().map(|profiler| profiler.span(name))
    }

    pub(crate) fn profile_span_with_args(
        &self,
        name: impl Into<String>,
        args: std::collections::BTreeMap<String, serde_json::Value>,
    ) -> Option<crate::GutchProfileSpan> {
        self.profiler
            .as_ref()
            .map(|profiler| profiler.span_with_args(name, args))
    }

    pub(crate) fn profile_event_with_args(
        &self,
        name: impl Into<String>,
        args: std::collections::BTreeMap<String, serde_json::Value>,
    ) {
        if let Some(profiler) = &self.profiler {
            profiler.event_with_args(name, args);
        }
    }

    pub(crate) fn progress(&self) -> &Option<Arc<dyn GutchProgressSink>> {
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
pub(crate) fn emit_progress(
    sink: &Option<std::sync::Arc<dyn GutchProgressSink>>,
    event: GutchProgressEvent,
) {
    if let Some(s) = sink {
        s.emit(event);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::{GutchCancellationHook, GutchOperationControl};

    #[derive(Debug)]
    struct TestCancellation(AtomicBool);

    impl GutchCancellationHook for TestCancellation {
        fn is_cancelled(&self) -> bool {
            self.0.load(Ordering::Acquire)
        }
    }

    #[test]
    fn operation_control_returns_typed_cancellation() {
        let cancellation = Arc::new(TestCancellation(AtomicBool::new(false)));
        let control = GutchOperationControl::default().with_cancellation(cancellation.clone());

        assert!(control.checkpoint().is_ok());
        cancellation.0.store(true, Ordering::Release);
        assert!(matches!(control.checkpoint(), Err(crate::Error::Cancelled)));
    }
}

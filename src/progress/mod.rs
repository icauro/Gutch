use serde::Serialize;

/// Identifies which gutch operation is emitting progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum GutchOperation {
    TargetInsert,
    TargetDelete,
    SyncUp,
    TargetSourcesSync,
    Fetch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GutchProgressPhase {
    Prepare,
    RepoManifest,
    TargetManifest,
    ChunkIndex,
    Bundle,
    Json,
    Done,
    ScanTargets,
    LoadManifests,
    ScanBundles,
    DeleteOrphans,
    RemoveManifest,
    Scan,
    Build,
    Planning,
    RemoveTargets,
    UploadRepoManifest,
    UploadChunkIndex,
    UploadTargetManifest,
    UploadBundle,
    Prune,
    Insert,
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
    /// Current progress counter (files scanned, bundles built, targets synced, etc.).
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

/// Helper to emit if a sink is present.
pub(crate) fn emit_progress(
    sink: &Option<std::sync::Arc<dyn GutchProgressSink>>,
    event: GutchProgressEvent,
) {
    if let Some(s) = sink {
        s.emit(event);
    }
}

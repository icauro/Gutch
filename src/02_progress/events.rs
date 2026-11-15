//! Progress events, warnings and the sinks that receive them.

use super::feedback::LiveFeedback;
use crate::error::promotion_diagnostics::DirectoryPromotionDiagnostic;
use serde::Serialize;
use std::fmt::Debug;
use std::sync::Arc;

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

/// A single unified progress event for all gutch operations.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressEvent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feedback: Option<LiveFeedback>,
    /// Which operation is running.
    pub operation: OperationKind,
    /// Current phase within the operation.
    pub phase: ProgressPhase,
    /// Human-readable message for the current step.
    pub message: String,
    /// Current progress counter (files scanned, bundles built, snapshots synced, etc.).
    pub current: usize,
    /// Total expected count for this phase.
    pub total: usize,
    /// Bytes transferred so far (for network operations).
    pub transferred_bytes: u64,
    /// Total bytes expected (for network operations).
    pub total_bytes: u64,
    /// Optional extra detail (current file name, bundle id, etc.).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// True when the engine has emitted its final progress stage. Errors still return through Result.
    pub done: bool,
}

/// Trait for receiving progress events. Implemented by CLI (indicatif) and Tauri (event emitter).
pub trait ProgressSink: Send + Sync + Debug {
    fn emit(&self, event: ProgressEvent);
}

/// Non-fatal condition reported while an operation still succeeds.
#[derive(Debug, Serialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum OperationWarning {
    /// The new directory is published, but the previous one could not be removed.
    DirectoryCleanupFailed(Box<DirectoryPromotionDiagnostic>),
}

impl std::fmt::Display for OperationWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OperationWarning::DirectoryCleanupFailed(_) => {
                f.write_str("directory promotion committed; cleanup failed")
            }
        }
    }
}

/// Receives non-fatal warnings. Without a sink, warnings are dropped.
pub trait WarningSink: Send + Sync + Debug {
    fn warn(&self, warning: OperationWarning);
}

/// Helper to emit if a sink is present.
pub(crate) fn emit_progress(sink: &Option<Arc<dyn ProgressSink>>, event: ProgressEvent) {
    if let Some(s) = sink {
        s.emit(event);
    }
}

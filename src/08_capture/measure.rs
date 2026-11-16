//! Fresh-capture storage measurement for directory or single-file inputs.
use crate::capture::build::pipeline::build_directory;
use crate::capture::build::types::BuildConfig;
use crate::capture::execute::prepare_input_path;
use crate::manifest::SnapshotTagSet;
use crate::progress::OperationControl;
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;

/// A source to measure and the tags its capture would carry.
#[derive(Debug, Clone, Default)]
pub struct SnapshotMeasureRequest {
    pub source_dir: PathBuf,
    pub tags: SnapshotTagSet,
    pub control: OperationControl,
}

/// Artifact sizes a fresh capture with the default packaging presets would
/// store, ignoring any repository reuse.
#[derive(Debug, Clone, Serialize)]
pub struct SnapshotMeasureResult {
    pub file_count: usize,
    pub logical_bytes: u64,
    pub unique_chunk_count: usize,
    pub unique_chunk_bytes: u64,
    pub encoded_artifact_bytes: u64,
    pub artifact_count: usize,
    pub bundle_count: usize,
}

/// Builds the source in memory and measures the artifacts it would produce.
pub fn measure_snapshot(request: &SnapshotMeasureRequest) -> crate::Result<SnapshotMeasureResult> {
    request.control.checkpoint()?;
    let _measure_span = request.control.profile_span("snapshot.measure");
    let input = prepare_input_path(&request.source_dir)?;
    let (result, _) = build_directory(&input.dir, &BuildConfig::for_snapshot(&request.tags))?;
    request.control.checkpoint()?;
    let mut chunks = HashMap::new();
    for file in result.snapshot_manifest.files() {
        for chunk in &file.chunks {
            chunks.insert(&chunk.chunk_hash, chunk.raw_size);
        }
    }
    Ok(SnapshotMeasureResult {
        file_count: result.snapshot_manifest.file_count(),
        logical_bytes: result.snapshot_manifest.total_logical_size,
        unique_chunk_count: chunks.len(),
        unique_chunk_bytes: chunks.values().map(|size| u64::from(*size)).sum(),
        encoded_artifact_bytes: result.bundles.iter().map(|bundle| bundle.size).sum::<u64>()
            + result
                .loose_chunks
                .iter()
                .map(|chunk| chunk.compressed_size)
                .sum::<u64>(),
        artifact_count: result.bundles.len() + result.loose_chunks.len(),
        bundle_count: result.bundles.len(),
    })
}

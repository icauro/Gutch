//! Fresh-capture storage measurement for directory or single-file inputs.
use crate::capture::build::pipeline::build_directory;
use crate::capture::build::types::BuildConfig;
use crate::capture::execute::prepare_input_path;
use crate::manifest::SnapshotTagSet;
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;

/// Artifact sizes a fresh capture with the default packaging presets would
/// store, ignoring any repository reuse.
#[derive(Debug, Clone, Serialize)]
pub struct SnapshotMeasurement {
    pub file_count: usize,
    pub logical_bytes: u64,
    pub unique_chunk_count: usize,
    pub unique_chunk_bytes: u64,
    pub encoded_artifact_bytes: u64,
    pub artifact_count: usize,
    pub bundle_count: usize,
}

/// Builds `source` in memory and measures the artifacts it would produce.
pub fn measure_snapshot(
    source: &Path,
    tags: &SnapshotTagSet,
) -> crate::Result<SnapshotMeasurement> {
    let input = prepare_input_path(source)?;
    let (result, _) = build_directory(&input.dir, &BuildConfig::for_snapshot(tags))?;
    let mut chunks = HashMap::new();
    for file in result.snapshot_manifest.files() {
        for chunk in &file.chunks {
            chunks.insert(&chunk.chunk_hash, chunk.raw_size);
        }
    }
    Ok(SnapshotMeasurement {
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

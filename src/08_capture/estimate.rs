//! Fresh reconstruction estimation for directory or single-file inputs.
use crate::capture::build::pipeline::build_directory;
use crate::capture::build::types::BuildConfig;
use crate::capture::execute::prepare_input_path;
use crate::manifest::Estimate;
use crate::manifest::SnapshotTagSet;
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug)]
pub struct SnapshotEstimateResult {
    pub file_count: usize,
    pub total_logical_size: u64,
    pub estimate: Estimate,
}

pub fn estimate_snapshot(
    source: &Path,
    tags: &SnapshotTagSet,
) -> crate::Result<SnapshotEstimateResult> {
    let input = prepare_input_path(source)?;
    let (result, _) = build_directory(&input.dir, &BuildConfig::for_snapshot(tags))?;
    let mut chunks = HashMap::new();
    for file in result.snapshot_manifest.files() {
        for chunk in &file.chunks {
            chunks.insert(&chunk.chunk_hash, chunk.raw_size);
        }
    }
    let estimate = Estimate {
        unique_chunk_bytes: chunks.values().map(|size| u64::from(*size)).sum(),
        unique_chunk_count: chunks.len(),
        encoded_artifact_bytes: result.bundles.iter().map(|bundle| bundle.size).sum::<u64>()
            + result
                .loose_chunks
                .iter()
                .map(|chunk| chunk.compressed_size)
                .sum::<u64>(),
        artifact_count: result.bundles.len() + result.loose_chunks.len(),
        bundle_count: result.bundles.len(),
    };
    Ok(SnapshotEstimateResult {
        file_count: result.snapshot_manifest.file_count(),
        total_logical_size: result.snapshot_manifest.total_logical_size,
        estimate,
    })
}

use std::collections::HashSet;

use crate::Result;
use crate::artifact::hash::ChunkHash;
use crate::manifest;
use crate::manifest::snapshot::SnapshotManifest;
use crate::manifest::typed::typed_file_manifest;

pub(crate) fn estimate_build(
    snapshot_manifest: &SnapshotManifest,
    available_chunks: &HashSet<ChunkHash>,
    available_disk_space: u64,
) -> Result<manifest::Estimate> {
    let mut missing_chunks: HashSet<ChunkHash> = HashSet::new();
    let mut missing_logical: u64 = 0;

    for file_manifest in snapshot_manifest.files() {
        let typed_file_manifest = typed_file_manifest(file_manifest)?;
        for chunk_ref in &typed_file_manifest.chunks {
            if !available_chunks.contains(&chunk_ref.hash) {
                missing_chunks.insert(chunk_ref.hash);
                missing_logical += chunk_ref.raw_size;
            }
        }
    }

    let estimate_ratio = 0.5;
    let missing_compressed = (missing_logical as f64 * estimate_ratio) as u64;
    let bundle_estimate = if missing_chunks.is_empty() {
        0
    } else {
        1 + (missing_chunks.len() as u64 / 2000)
    };

    Ok(manifest::Estimate {
        missing_logical_bytes: missing_logical,
        missing_compressed_bytes: missing_compressed,
        missing_chunk_count: missing_chunks.len() as u64,
        required_bundle_count: bundle_estimate,
        range_request_count: missing_chunks.len() as u64,
        local_cache_hits: 0,
        available_disk_space,
        fragmentation_score: bundle_estimate as f64,
    })
}

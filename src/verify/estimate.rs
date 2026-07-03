use std::collections::HashSet;

use crate::Result;
use crate::manifest::{self, target::TargetManifest};

pub fn estimate_build(
    target_manifest: &TargetManifest,
    available_chunks: &HashSet<String>,
    available_disk_space: u64,
) -> Result<manifest::Estimate> {
    let mut missing_chunks: HashSet<String> = HashSet::new();
    let mut missing_logical: u64 = 0;

    for file_manifest in &target_manifest.files {
        for chunk_ref in &file_manifest.chunks {
            if !available_chunks.contains(&chunk_ref.chunk_hash) {
                missing_chunks.insert(chunk_ref.chunk_hash.clone());
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

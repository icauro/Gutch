use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rayon::prelude::*;

use crate::bundle;
use crate::runtime::{RuntimePlan, install_in_thread_pool};
use crate::Result;

use super::super::types::{
    BundleAccessPlan, BundleAccessStrategy, BundleScanInfo, ChunkLocation,
    ReconstructTimings, StagedBundlePayload,
};

// ---------------------------------------------------------------------------
// Staged bundle access strategy
// ---------------------------------------------------------------------------

pub(in crate::reconstruct) fn stage_reconstruct_bundle_access(
    locations: &HashMap<String, ChunkLocation>,
    bundle_scan_info: &HashMap<PathBuf, BundleScanInfo>,
    runtime_plan: &RuntimePlan,
    timings: &mut ReconstructTimings,
) -> Result<BundleAccessPlan> {
    let mut strategies = HashMap::new();
    let mut needed_bundle_paths: Vec<PathBuf> = locations
        .values()
        .map(|location| location.bundle_path.clone())
        .collect();
    needed_bundle_paths.sort();
    needed_bundle_paths.dedup();

    let mut bundle_compressed_paths = Vec::new();
    for bundle_path in needed_bundle_paths {
        let scan_info = bundle_scan_info.get(&bundle_path).ok_or_else(|| {
            crate::Error::Reconstruct(format!(
                "missing bundle scan metadata for {}",
                bundle_path.display()
            ))
        })?;
        if scan_info.compression_unit == crate::manifest::COMPRESSION_UNIT_BUNDLE {
            bundle_compressed_paths.push(bundle_path);
        } else {
            strategies.insert(bundle_path, BundleAccessStrategy::DirectChunkRead);
        }
    }

    if bundle_compressed_paths.is_empty() {
        return Ok(BundleAccessPlan {
            strategies,
            _staging_dir: None,
        });
    }

    let stage_in_memory = should_stage_bundle_payloads_in_memory(
        &bundle_compressed_paths,
        bundle_scan_info,
        runtime_plan,
    );
    let staging_dir = if stage_in_memory {
        None
    } else {
        Some(tempfile::tempdir().map_err(|err| {
            crate::Error::Reconstruct(format!("failed to create bundle staging dir: {}", err))
        })?)
    };
    let staged_results = install_in_thread_pool(runtime_plan.worker_count, || {
        bundle_compressed_paths
            .par_iter()
            .enumerate()
            .map(|(index, bundle_path)| {
                stage_bundle_raw_payload(
                    bundle_path,
                    staging_dir.as_ref().map(|dir| dir.path()),
                    index,
                )
            })
            .collect::<Vec<_>>()
    })?;

    for staged in staged_results {
        let (bundle_path, staged_payload, compressed_size, staged_raw_size, stage_time) = staged?;
        timings.bundle_read += stage_time;
        timings.bundle_read_compressed_bytes += compressed_size;
        timings.bundle_read_decompressed_bytes += staged_raw_size;
        let strategy = match staged_payload {
            StagedBundlePayload::TempFile(staged_raw_path) => {
                BundleAccessStrategy::StagedBundleRaw { staged_raw_path }
            }
            StagedBundlePayload::Memory(raw_payload) => {
                BundleAccessStrategy::StagedBundleMemory { raw_payload }
            }
        };
        strategies.insert(bundle_path, strategy);
    }

    Ok(BundleAccessPlan {
        strategies,
        _staging_dir: staging_dir,
    })
}

fn stage_bundle_raw_payload(
    bundle_path: &Path,
    staging_root: Option<&Path>,
    index: usize,
) -> Result<(PathBuf, StagedBundlePayload, u64, u64, Duration)> {
    let stage_start = Instant::now();
    let file = std::fs::File::open(bundle_path)?;
    let mut reader = bundle::BundleReader::open(file)?;
    let payload = if let Some(staging_root) = staging_root {
        let bundle_stem = bundle_path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("bundle");
        let staged_raw_path = staging_root.join(format!("{}-{}.raw", index, bundle_stem));
        let mut staged_file = std::fs::File::create(&staged_raw_path)?;
        let staged_raw_size = reader.stage_bundle_payload_to(&mut staged_file)?;
        (
            StagedBundlePayload::TempFile(staged_raw_path),
            staged_raw_size,
        )
    } else {
        let mut raw_payload = Vec::new();
        let staged_raw_size = reader.stage_bundle_payload_to(&mut raw_payload)?;
        (
            StagedBundlePayload::Memory(Arc::from(raw_payload)),
            staged_raw_size,
        )
    };
    Ok((
        bundle_path.to_path_buf(),
        payload.0,
        reader.payload_size(),
        payload.1,
        stage_start.elapsed(),
    ))
}

pub(in crate::reconstruct) fn should_stage_bundle_payloads_in_memory(
    bundle_paths: &[PathBuf],
    bundle_scan_info: &HashMap<PathBuf, BundleScanInfo>,
    runtime_plan: &RuntimePlan,
) -> bool {
    if runtime_plan.effective_memory_mode != crate::runtime::EffectiveMemoryMode::High {
        return false;
    }

    let estimated_raw_bytes = bundle_paths
        .iter()
        .filter_map(|path| bundle_scan_info.get(path))
        .map(|info| info.raw_payload_size)
        .sum::<u64>();
    let memory_budget_bytes = runtime_plan.memory_budget_mib.saturating_mul(1024 * 1024);
    estimated_raw_bytes > 0 && estimated_raw_bytes <= memory_budget_bytes / 2
}

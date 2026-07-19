use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use rayon::prelude::*;

use crate::bundle;
use crate::hash::ChunkHash;
use crate::manifest::target::TargetManifest;
use crate::path_guard::resolve_manifest_relative_path;
use crate::progress::GutchOperationControl;
use crate::runtime::{RuntimePlan, install_in_thread_pool};
use crate::{Error, Result};

use super::super::types::{
    BundleScanInfo, ChunkLocation, ChunkWriteTarget, ReconstructOptions, ReconstructTimings,
    typed_chunk_refs,
};
use super::orchestrator::verify_chunk_hash_with_timings;

// ---------------------------------------------------------------------------
// Direct bundle-to-file restore strategy (currently disabled by default)
// ---------------------------------------------------------------------------

pub(in crate::reconstruct) fn should_use_direct_bundle_to_file_restore(
    local_chunks: &HashMap<ChunkHash, Vec<u8>>,
    bundle_scan_info: &HashMap<PathBuf, BundleScanInfo>,
    runtime_plan: &RuntimePlan,
    options: ReconstructOptions,
) -> bool {
    if runtime_plan.effective_memory_mode != crate::runtime::EffectiveMemoryMode::High {
        return false;
    }
    if options.verify_files || !local_chunks.is_empty() || bundle_scan_info.is_empty() {
        return false;
    }
    bundle_scan_info
        .values()
        .all(|info| info.compression_unit == crate::manifest::CompressionUnit::Bundle)
}

#[cfg(test)]
pub(in crate::reconstruct) fn should_use_direct_bundle_to_file_restore_experimental(
    local_chunks: &HashMap<ChunkHash, Vec<u8>>,
    bundle_scan_info: &HashMap<PathBuf, BundleScanInfo>,
    runtime_plan: &RuntimePlan,
    options: ReconstructOptions,
) -> bool {
    if runtime_plan.effective_memory_mode != crate::runtime::EffectiveMemoryMode::High {
        return false;
    }
    if options.verify_files || !local_chunks.is_empty() || bundle_scan_info.is_empty() {
        return false;
    }
    bundle_scan_info
        .values()
        .all(|info| info.compression_unit == crate::manifest::CompressionUnit::Bundle)
}

// ---------------------------------------------------------------------------
// Direct reconstruction pipeline
// ---------------------------------------------------------------------------

pub(in crate::reconstruct) fn direct_reconstruct_bundle_compressed_to_files(
    target_manifest: &TargetManifest,
    locations: &HashMap<ChunkHash, ChunkLocation>,
    output_dir: &Path,
    timings: &mut ReconstructTimings,
    worker_count: usize,
    options: ReconstructOptions,
    control: &GutchOperationControl,
) -> Result<()> {
    control.checkpoint()?;
    let write_targets = build_chunk_write_targets(target_manifest, output_dir, control)?;
    prepare_direct_output_files(target_manifest, output_dir, control)?;

    let mut bundle_locations: HashMap<PathBuf, Vec<(ChunkHash, ChunkLocation)>> = HashMap::new();
    for (chunk_hash, location) in locations {
        control.checkpoint()?;
        bundle_locations
            .entry(location.bundle_path.clone())
            .or_default()
            .push((*chunk_hash, location.clone()));
    }
    let mut bundle_locations = bundle_locations.into_iter().collect::<Vec<_>>();
    bundle_locations.sort_by(|(left, _), (right, _)| left.cmp(right));

    let bundle_results = install_in_thread_pool(worker_count, || {
        bundle_locations
            .par_iter()
            .map(|(bundle_path, chunk_locations)| {
                control.checkpoint()?;
                direct_reconstruct_one_bundle(
                    bundle_path,
                    chunk_locations,
                    &write_targets,
                    options,
                    control,
                )
            })
            .collect::<Vec<_>>()
    })?;

    for result in bundle_results {
        control.checkpoint()?;
        let bundle_timings = result?;
        timings.accumulate(&bundle_timings);
    }

    Ok(())
}

fn build_chunk_write_targets(
    target_manifest: &TargetManifest,
    output_dir: &Path,
    control: &GutchOperationControl,
) -> Result<HashMap<ChunkHash, Vec<ChunkWriteTarget>>> {
    let mut write_targets: HashMap<ChunkHash, Vec<ChunkWriteTarget>> = HashMap::new();
    for fm in &target_manifest.files {
        control.checkpoint()?;
        let output_path = resolve_manifest_relative_path(output_dir, &fm.path)?;
        let mut file_offset = 0u64;
        for chunk_ref in typed_chunk_refs(fm)? {
            control.checkpoint()?;
            let chunk_hash = chunk_ref.hash;
            write_targets
                .entry(chunk_hash)
                .or_default()
                .push(ChunkWriteTarget {
                    output_path: output_path.clone(),
                    file_offset,
                    raw_size: chunk_ref.raw_size,
                });
            file_offset = file_offset.saturating_add(chunk_ref.raw_size);
        }
    }
    Ok(write_targets)
}

fn prepare_direct_output_files(
    target_manifest: &TargetManifest,
    output_dir: &Path,
    control: &GutchOperationControl,
) -> Result<()> {
    for fm in &target_manifest.files {
        control.checkpoint()?;
        let output_path = resolve_manifest_relative_path(output_dir, &fm.path)?;
        if let Some(parent) = output_path.parent() {
            crate::helpers::create_dir_all(parent)?;
        }
        let file = std::fs::File::create(&output_path)?;
        file.set_len(fm.size)?;
    }
    Ok(())
}

fn direct_reconstruct_one_bundle(
    bundle_path: &Path,
    chunk_locations: &[(ChunkHash, ChunkLocation)],
    write_targets: &HashMap<ChunkHash, Vec<ChunkWriteTarget>>,
    options: ReconstructOptions,
    control: &GutchOperationControl,
) -> Result<ReconstructTimings> {
    control.checkpoint()?;
    let mut timings = ReconstructTimings::default();
    let stage_start = Instant::now();
    let file = std::fs::File::open(bundle_path)?;
    let mut reader = bundle::BundleReader::open(file)?;
    let mut raw_payload = Vec::new();
    let staged_raw_size = reader.stage_bundle_payload_to(&mut raw_payload)?;
    timings.bundle_read += stage_start.elapsed();
    timings.bundle_read_compressed_bytes += reader.payload_size();
    timings.bundle_read_decompressed_bytes += staged_raw_size;
    let mut output_files: HashMap<PathBuf, std::fs::File> = HashMap::new();

    for (chunk_hash, location) in chunk_locations {
        control.checkpoint()?;
        let chunk_data =
            direct_bundle_chunk_slice(&raw_payload, location, *chunk_hash, &mut timings, options)?;
        let Some(targets) = write_targets.get(chunk_hash) else {
            continue;
        };
        for target in targets {
            control.checkpoint()?;
            if target.raw_size != location.raw_size {
                return Err(Error::Reconstruct(format!(
                    "write target size {} does not match chunk size {} for {}",
                    target.raw_size, location.raw_size, chunk_hash
                )));
            }
            let t = Instant::now();
            let output = match output_files.entry(target.output_path.clone()) {
                std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(OpenOptions::new().write(true).open(&target.output_path)?)
                }
            };
            output.seek(SeekFrom::Start(target.file_offset))?;
            output.write_all(chunk_data)?;
            timings.file_write += t.elapsed();
            timings.file_write_bytes += chunk_data.len() as u64;
        }
    }

    Ok(timings)
}

fn direct_bundle_chunk_slice<'a>(
    raw_payload: &'a [u8],
    location: &ChunkLocation,
    expected_chunk_hash: ChunkHash,
    timings: &mut ReconstructTimings,
    options: ReconstructOptions,
) -> Result<&'a [u8]> {
    let t = Instant::now();
    let chunk_data = slice_bundle_raw_payload(raw_payload, location, "direct")?;
    timings.bundle_read += t.elapsed();
    verify_chunk_hash_with_timings(chunk_data, expected_chunk_hash, options, timings)?;
    Ok(chunk_data)
}

// ---------------------------------------------------------------------------
// Shared helper — also used by staging.rs
// ---------------------------------------------------------------------------

pub(in crate::reconstruct) fn slice_bundle_raw_payload<'a>(
    raw_payload: &'a [u8],
    location: &ChunkLocation,
    mode: &str,
) -> Result<&'a [u8]> {
    let start = location.raw_offset as usize;
    let end = start.saturating_add(location.raw_size as usize);
    if end > raw_payload.len() {
        return Err(Error::Reconstruct(format!(
            "bundle raw span {}..{} exceeds {} payload size {}",
            start,
            end,
            mode,
            raw_payload.len()
        )));
    }
    Ok(&raw_payload[start..end])
}

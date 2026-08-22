use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use rayon::prelude::*;

use crate::bundle;
use crate::hash::ChunkHash;
use crate::manifest::CompressionProfile;
use crate::manifest::snapshot::SnapshotManifest;
use crate::path_guard::resolve_manifest_relative_path;
use crate::progress::GutchOperationControl;
use crate::runtime::{RuntimePlan, run_in_thread_pool};
use crate::{Error, Result};

use super::super::types::{
    BundleScanInfo, ChunkLocation, ChunkWriteSnapshot, ReconstructOptions, ReconstructTimings,
    typed_chunk_refs,
};
use super::orchestrator::verify_chunk_hash_with_timings;

// ---------------------------------------------------------------------------
// Direct bundle-to-file reconstruct strategy (currently disabled by default)
// ---------------------------------------------------------------------------

pub(in crate::reconstruct) fn should_use_direct_bundle_to_file_reconstruct(
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
pub(in crate::reconstruct) fn should_use_direct_bundle_to_file_reconstruct_experimental(
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
    snapshot_manifest: &SnapshotManifest,
    locations: &HashMap<ChunkHash, ChunkLocation>,
    output_dir: &Path,
    compression_profile: &CompressionProfile,
    timings: &mut ReconstructTimings,
    worker_count: usize,
    options: ReconstructOptions,
    control: &GutchOperationControl,
) -> Result<()> {
    control.checkpoint()?;
    let write_snapshots = build_chunk_write_snapshots(snapshot_manifest, output_dir, control)?;
    prepare_direct_output_files(snapshot_manifest, output_dir, control)?;

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

    let bundle_results = run_in_thread_pool(worker_count, || {
        bundle_locations
            .par_iter()
            .map(|(bundle_path, chunk_locations)| {
                control.checkpoint()?;
                direct_reconstruct_one_bundle(
                    bundle_path,
                    chunk_locations,
                    &write_snapshots,
                    compression_profile,
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

fn build_chunk_write_snapshots(
    snapshot_manifest: &SnapshotManifest,
    output_dir: &Path,
    control: &GutchOperationControl,
) -> Result<HashMap<ChunkHash, Vec<ChunkWriteSnapshot>>> {
    let mut write_snapshots: HashMap<ChunkHash, Vec<ChunkWriteSnapshot>> = HashMap::new();
    for fm in snapshot_manifest.files() {
        control.checkpoint()?;
        let output_path = resolve_manifest_relative_path(output_dir, &fm.path)?;
        let mut file_offset = 0u64;
        for chunk_ref in typed_chunk_refs(fm)? {
            control.checkpoint()?;
            let chunk_hash = chunk_ref.hash;
            write_snapshots
                .entry(chunk_hash)
                .or_default()
                .push(ChunkWriteSnapshot {
                    output_path: output_path.clone(),
                    file_offset,
                    raw_size: chunk_ref.raw_size,
                });
            file_offset = file_offset.saturating_add(chunk_ref.raw_size);
        }
    }
    Ok(write_snapshots)
}

fn prepare_direct_output_files(
    snapshot_manifest: &SnapshotManifest,
    output_dir: &Path,
    control: &GutchOperationControl,
) -> Result<()> {
    for fm in snapshot_manifest.files() {
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
    write_snapshots: &HashMap<ChunkHash, Vec<ChunkWriteSnapshot>>,
    compression_profile: &CompressionProfile,
    options: ReconstructOptions,
    control: &GutchOperationControl,
) -> Result<ReconstructTimings> {
    control.checkpoint()?;
    let mut timings = ReconstructTimings::default();
    let stage_start = Instant::now();
    let file = std::fs::File::open(bundle_path)?;
    let mut reader = bundle::BundleReader::open(file, compression_profile)?;
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
        let Some(snapshots) = write_snapshots.get(chunk_hash) else {
            continue;
        };
        for snapshot in snapshots {
            control.checkpoint()?;
            if snapshot.raw_size != location.raw_size {
                return Err(Error::Reconstruct(format!(
                    "write snapshot size {} does not match chunk size {} for {}",
                    snapshot.raw_size, location.raw_size, chunk_hash
                )));
            }
            let t = Instant::now();
            let output = match output_files.entry(snapshot.output_path.clone()) {
                std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(OpenOptions::new().write(true).open(&snapshot.output_path)?)
                }
            };
            output.seek(SeekFrom::Start(snapshot.file_offset))?;
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

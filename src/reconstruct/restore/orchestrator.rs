use std::collections::{HashMap, HashSet};
use std::io::BufWriter;
use std::io::Write;
use std::path::Path;
use std::time::Instant;

use rayon::prelude::*;

use crate::bundle;
use crate::compress::decompress_data;
use crate::hash::{ChunkHash, hash_hex};
use crate::manifest::{ArtifactStorageMode, CompressionProfile, target::TargetManifest};
use crate::path_guard::resolve_manifest_relative_path;
use crate::repo::{GutchRepoPaths, chunk_path};
use crate::runtime::{
    RuntimePlan, RuntimeResourceHints, install_in_thread_pool, plan_reconstruct_runtime,
};
use crate::{Error, Result};

use super::super::types::{
    BundleAccessPlan, BundleAccessStrategy, ChunkLocation, LoadedChunk, ReconstructOptions,
    ReconstructTimings, typed_chunk_refs,
};
use super::direct::{
    direct_reconstruct_bundle_compressed_to_files, should_use_direct_bundle_to_file_restore,
};
use super::staging::stage_reconstruct_bundle_access;

// ---------------------------------------------------------------------------
// Entry preparation
// ---------------------------------------------------------------------------

pub(in crate::reconstruct) fn prepare_reconstruct(
    target_manifest: &TargetManifest,
    output_dir: &Path,
    runtime_hints: &RuntimeResourceHints,
) -> Result<(HashSet<ChunkHash>, Instant, RuntimePlan, ReconstructTimings)> {
    let needed_chunks = crate::repo::required_chunk_hashes_typed(target_manifest)?;
    let total_start = Instant::now();
    let runtime_plan = plan_reconstruct_runtime(runtime_hints);
    let timings = ReconstructTimings {
        runtime_plan: Some(runtime_plan.clone()),
        ..ReconstructTimings::default()
    };
    crate::helpers::create_dir_all(output_dir)?;
    Ok((needed_chunks, total_start, runtime_plan, timings))
}

// ---------------------------------------------------------------------------
// Main orchestrator — bundle-path based reconstruction
// ---------------------------------------------------------------------------

pub(in crate::reconstruct) fn reconstruct_from_bundle_paths_and_local_chunks(
    target_manifest: &TargetManifest,
    bundle_paths: &[std::path::PathBuf],
    needed_chunks: &HashSet<ChunkHash>,
    local_chunks: HashMap<ChunkHash, Vec<u8>>,
    output_dir: &Path,
    runtime_plan: RuntimePlan,
    total_start: Instant,
    mut timings: ReconstructTimings,
    options: ReconstructOptions,
) -> Result<((), ReconstructTimings)> {
    let missing_chunks: HashSet<ChunkHash> = needed_chunks
        .iter()
        .filter(|chunk_hash| !local_chunks.contains_key(*chunk_hash))
        .copied()
        .collect();
    let mut locations: HashMap<ChunkHash, ChunkLocation> =
        HashMap::with_capacity(needed_chunks.len());
    let mut bundle_scan_info: HashMap<std::path::PathBuf, super::super::types::BundleScanInfo> =
        HashMap::new();

    for bundle_path in bundle_paths {
        let file = std::fs::File::open(bundle_path)?;
        let reader = bundle::BundleReader::open(file)?;
        bundle_scan_info.insert(
            bundle_path.clone(),
            super::super::types::BundleScanInfo {
                compression_unit: reader.compression_unit(),
                raw_payload_size: estimate_bundle_raw_payload_size(reader.entries()),
            },
        );
        for (index, entry) in reader.entries().iter().enumerate() {
            let chunk_hash = reader.chunk_hashes()[index];
            if missing_chunks.contains(&chunk_hash) {
                locations
                    .entry(chunk_hash)
                    .or_insert_with(|| ChunkLocation {
                        bundle_path: bundle_path.clone(),
                        index,
                        compressed_size: entry.compressed_size,
                        raw_offset: entry.raw_offset,
                        raw_size: entry.raw_size,
                    });
            }
        }

        if locations.len() == missing_chunks.len() {
            break;
        }
    }

    if should_use_direct_bundle_to_file_restore(
        &local_chunks,
        &bundle_scan_info,
        &runtime_plan,
        options,
    ) {
        direct_reconstruct_bundle_compressed_to_files(
            target_manifest,
            &locations,
            output_dir,
            &mut timings,
            runtime_plan.worker_count,
            options,
        )?;
        timings.total = total_start.elapsed();
        return Ok(((), timings));
    }

    let access_plan = stage_reconstruct_bundle_access(
        &locations,
        &bundle_scan_info,
        &runtime_plan,
        &mut timings,
    )?;

    reconstruct_files_from_locations_and_chunks(
        target_manifest,
        &locations,
        &access_plan,
        &local_chunks,
        output_dir,
        &mut timings,
        runtime_plan.worker_count,
        options,
    )?;
    timings.total = total_start.elapsed();
    Ok(((), timings))
}

// ---------------------------------------------------------------------------
// Loose chunk reading (repo-based reconstruction)
// ---------------------------------------------------------------------------

pub(in crate::reconstruct) fn read_loose_repo_chunks(
    repo_paths: &GutchRepoPaths,
    chunk_index: &crate::manifest::ChunkIndex,
    compression_profile: &CompressionProfile,
    needed_chunks: &HashSet<ChunkHash>,
    options: ReconstructOptions,
    worker_count: usize,
    timings: &mut ReconstructTimings,
) -> Result<HashMap<ChunkHash, Vec<u8>>> {
    let typed_chunk_index_entries = crate::repo::chunk_index_entries_typed(chunk_index)?;
    let loaded_chunks = install_in_thread_pool(worker_count, || {
        needed_chunks
            .par_iter()
            .filter_map(|chunk_hash| {
                let index_entry = typed_chunk_index_entries.get(chunk_hash)?;
                if index_entry.storage_mode != ArtifactStorageMode::LooseChunks {
                    return None;
                }
                Some(read_loose_repo_chunk(
                    repo_paths,
                    compression_profile,
                    chunk_hash,
                    options,
                ))
            })
            .collect::<Vec<_>>()
    })?;

    let mut chunk_map = HashMap::with_capacity(loaded_chunks.len());
    for loaded_chunk in loaded_chunks {
        let loaded_chunk = loaded_chunk?;
        timings.bundle_read += loaded_chunk.read_duration;
        timings.bundle_read_compressed_bytes += loaded_chunk.compressed_bytes;
        timings.bundle_read_decompressed_bytes += loaded_chunk.decompressed_bytes;
        chunk_map.insert(loaded_chunk.chunk_hash, loaded_chunk.data);
    }
    Ok(chunk_map)
}

fn read_loose_repo_chunk(
    repo_paths: &GutchRepoPaths,
    compression_profile: &CompressionProfile,
    chunk_hash: &ChunkHash,
    options: ReconstructOptions,
) -> Result<LoadedChunk> {
    let chunk_hash_hex = chunk_hash.to_hex();
    let path = chunk_path(repo_paths, &chunk_hash_hex);
    let read_start = Instant::now();
    let stored = crate::helpers::read(&path)?;
    let read_duration = read_start.elapsed();
    let chunk_data = decompress_data(&stored, compression_profile.compression)?;
    verify_chunk_hash(&chunk_data, *chunk_hash, options)?;
    Ok(LoadedChunk {
        chunk_hash: *chunk_hash,
        compressed_bytes: stored.len() as u64,
        decompressed_bytes: chunk_data.len() as u64,
        data: chunk_data,
        read_duration,
    })
}

// ---------------------------------------------------------------------------
// File reconstruction from chunk map (in-memory bundles path)
// ---------------------------------------------------------------------------

pub(in crate::reconstruct) fn rebuild_files_from_chunks(
    target_manifest: &TargetManifest,
    chunk_map: &HashMap<ChunkHash, Vec<u8>>,
    output_dir: &Path,
    timings: &mut ReconstructTimings,
) -> Result<()> {
    for fm in &target_manifest.files {
        let t = Instant::now();
        let mut file_data = Vec::with_capacity(fm.size as usize);

        for chunk_ref in typed_chunk_refs(fm)? {
            let chunk_hash = chunk_ref.hash;
            let chunk_data = chunk_map
                .get(&chunk_hash)
                .ok_or_else(|| Error::MissingChunk(chunk_ref.hash_hex.to_string()))?;

            file_data.extend_from_slice(chunk_data);
        }
        timings.file_rebuild += t.elapsed();
        timings.file_rebuild_bytes += file_data.len() as u64;

        if file_data.len() != fm.size as usize {
            return Err(Error::Reconstruct(format!(
                "reconstructed size {} does not match expected {} for {}",
                file_data.len(),
                fm.size,
                fm.path
            )));
        }

        let t = Instant::now();
        let actual_hash = hash_hex(&crate::hash::hash_chunk(&file_data));
        timings.file_verify += t.elapsed();
        timings.file_verify_bytes += file_data.len() as u64;
        if actual_hash != fm.full_file_hash {
            return Err(Error::HashMismatch {
                expected: fm.full_file_hash.clone(),
                actual: actual_hash,
            });
        }

        let output_path = resolve_manifest_relative_path(output_dir, &fm.path)?;
        if let Some(parent) = output_path.parent() {
            crate::helpers::create_dir_all(parent)?;
        }
        let t = Instant::now();
        crate::helpers::write(&output_path, &file_data)?;
        timings.file_write += t.elapsed();
        timings.file_write_bytes += file_data.len() as u64;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Per-file reconstruction (staged/direct access paths)
// ---------------------------------------------------------------------------

fn reconstruct_files_from_locations_and_chunks(
    target_manifest: &TargetManifest,
    locations: &HashMap<ChunkHash, ChunkLocation>,
    access_plan: &BundleAccessPlan,
    local_chunks: &HashMap<ChunkHash, Vec<u8>>,
    output_dir: &Path,
    timings: &mut ReconstructTimings,
    worker_count: usize,
    options: ReconstructOptions,
) -> Result<()> {
    let per_file = install_in_thread_pool(worker_count, || {
        target_manifest
            .files
            .par_iter()
            .map(|fm| {
                reconstruct_manifest_file(
                    fm,
                    locations,
                    access_plan,
                    local_chunks,
                    output_dir,
                    options,
                )
            })
            .collect::<Vec<_>>()
    })?;

    for result in per_file {
        let file_timings = result?;
        accumulate_reconstruct_timings(timings, &file_timings);
    }

    Ok(())
}

fn reconstruct_manifest_file(
    fm: &crate::manifest::FileManifest,
    locations: &HashMap<ChunkHash, ChunkLocation>,
    access_plan: &BundleAccessPlan,
    local_chunks: &HashMap<ChunkHash, Vec<u8>>,
    output_dir: &Path,
    options: ReconstructOptions,
) -> Result<ReconstructTimings> {
    let mut timings = ReconstructTimings::default();
    let output_path = resolve_manifest_relative_path(output_dir, &fm.path)?;
    if let Some(parent) = output_path.parent() {
        crate::helpers::create_dir_all(parent)?;
    }

    let mut output = BufWriter::new(std::fs::File::create(&output_path)?);
    let mut hasher = options.verify_files.then(blake3::Hasher::new);
    let mut written = 0u64;
    let mut staged_chunk_buffer = Vec::new();
    let mut bundle_readers: HashMap<std::path::PathBuf, bundle::BundleReader<std::fs::File>> =
        HashMap::new();
    let mut staged_raw_readers: HashMap<std::path::PathBuf, std::fs::File> = HashMap::new();

    for chunk_ref in typed_chunk_refs(fm)? {
        let chunk_hash = chunk_ref.hash;
        if let Some(local_chunk) = local_chunks.get(&chunk_hash) {
            write_reconstructed_chunk(
                &mut output,
                local_chunk,
                &mut hasher,
                &mut timings,
                &mut written,
            )?;
            continue;
        }

        let location = locations
            .get(&chunk_hash)
            .ok_or_else(|| Error::MissingChunk(chunk_ref.hash_hex.to_string()))?;
        let strategy = access_plan
            .strategies
            .get(&location.bundle_path)
            .ok_or_else(|| Error::MissingChunk(chunk_ref.hash_hex.to_string()))?;
        if let BundleAccessStrategy::StagedBundleMemory { raw_payload } = strategy {
            let chunk_data = read_staged_memory_chunk_for_reconstruct(
                raw_payload,
                location,
                chunk_hash,
                &mut timings,
                options,
            )?;
            write_reconstructed_chunk(
                &mut output,
                chunk_data,
                &mut hasher,
                &mut timings,
                &mut written,
            )?;
            continue;
        }

        match strategy {
            BundleAccessStrategy::DirectChunkRead => {
                let reader = match bundle_readers.entry(location.bundle_path.clone()) {
                    std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        let file = std::fs::File::open(&location.bundle_path)?;
                        entry.insert(bundle::BundleReader::open(file)?)
                    }
                };

                let t = Instant::now();
                let chunk_data = reader.read_chunk_bytes(location.index)?;
                timings.bundle_read += t.elapsed();
                timings.bundle_read_compressed_bytes += location.compressed_size;
                timings.bundle_read_decompressed_bytes += chunk_data.len() as u64;
                write_reconstructed_chunk(
                    &mut output,
                    chunk_data,
                    &mut hasher,
                    &mut timings,
                    &mut written,
                )?;
            }
            BundleAccessStrategy::StagedBundleRaw { staged_raw_path } => {
                use std::io::{Read, Seek, SeekFrom};

                let staged_file = match staged_raw_readers.entry(staged_raw_path.clone()) {
                    std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        entry.insert(std::fs::File::open(staged_raw_path)?)
                    }
                };

                staged_chunk_buffer.resize(location.raw_size as usize, 0);
                let t = Instant::now();
                staged_file.seek(SeekFrom::Start(location.raw_offset))?;
                staged_file.read_exact(&mut staged_chunk_buffer)?;
                timings.bundle_read += t.elapsed();
                timings.bundle_read_decompressed_bytes += location.raw_size;
                verify_chunk_hash(&staged_chunk_buffer, chunk_hash, options)?;
                write_reconstructed_chunk(
                    &mut output,
                    &staged_chunk_buffer,
                    &mut hasher,
                    &mut timings,
                    &mut written,
                )?;
            }
            BundleAccessStrategy::StagedBundleMemory { .. } => {
                return Err(Error::Reconstruct(
                    "in-memory bundle strategy must be handled by caller".to_string(),
                ));
            }
        }
    }

    if written != fm.size {
        return Err(Error::Reconstruct(format!(
            "reconstructed size {} does not match expected {} for {}",
            written, fm.size, fm.path
        )));
    }

    let t = Instant::now();
    output.flush()?;
    timings.file_write += t.elapsed();

    if let Some(hasher) = hasher {
        let t = Instant::now();
        let actual_hash = hash_hex(&hasher.finalize());
        timings.file_verify += t.elapsed();
        timings.file_verify_bytes += written;
        if actual_hash != fm.full_file_hash {
            return Err(Error::HashMismatch {
                expected: fm.full_file_hash.clone(),
                actual: actual_hash,
            });
        }
    }

    Ok(timings)
}

fn read_staged_memory_chunk_for_reconstruct<'a>(
    raw_payload: &'a [u8],
    location: &ChunkLocation,
    expected_chunk_hash: ChunkHash,
    timings: &mut ReconstructTimings,
    options: ReconstructOptions,
) -> Result<&'a [u8]> {
    use super::direct::slice_bundle_raw_payload;

    let chunk_data = slice_bundle_raw_payload(raw_payload, location, "staged")?;
    let t = Instant::now();
    timings.bundle_read += t.elapsed();
    timings.bundle_read_decompressed_bytes += location.raw_size;
    verify_chunk_hash(chunk_data, expected_chunk_hash, options)?;
    Ok(chunk_data)
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

pub(in crate::reconstruct) fn verify_chunk_hash(
    chunk_data: &[u8],
    expected_chunk_hash: ChunkHash,
    options: ReconstructOptions,
) -> Result<()> {
    if !options.verify_chunks {
        return Ok(());
    }

    let actual_hash = ChunkHash::from_blake3(crate::hash::hash_chunk(chunk_data));
    if actual_hash != expected_chunk_hash {
        return Err(Error::HashMismatch {
            expected: expected_chunk_hash.to_hex(),
            actual: actual_hash.to_hex(),
        });
    }
    Ok(())
}

fn write_reconstructed_chunk(
    output: &mut BufWriter<std::fs::File>,
    chunk_data: &[u8],
    hasher: &mut Option<blake3::Hasher>,
    timings: &mut ReconstructTimings,
    written: &mut u64,
) -> Result<()> {
    let t = Instant::now();
    output.write_all(chunk_data)?;
    timings.file_write += t.elapsed();
    timings.file_write_bytes += chunk_data.len() as u64;
    if let Some(hasher) = hasher {
        hasher.update(chunk_data);
    }
    *written += chunk_data.len() as u64;
    timings.file_rebuild_bytes += chunk_data.len() as u64;
    Ok(())
}

fn estimate_bundle_raw_payload_size(entries: &[crate::manifest::BundleTableEntry]) -> u64 {
    entries
        .iter()
        .map(|entry| entry.raw_offset.saturating_add(entry.raw_size))
        .max()
        .unwrap_or(0)
}

fn accumulate_reconstruct_timings(total: &mut ReconstructTimings, add: &ReconstructTimings) {
    total.bundle_read += add.bundle_read;
    total.bundle_read_compressed_bytes += add.bundle_read_compressed_bytes;
    total.bundle_read_decompressed_bytes += add.bundle_read_decompressed_bytes;
    total.file_rebuild += add.file_rebuild;
    total.file_rebuild_bytes += add.file_rebuild_bytes;
    total.file_verify += add.file_verify;
    total.file_verify_bytes += add.file_verify_bytes;
    total.file_write += add.file_write;
    total.file_write_bytes += add.file_write_bytes;
}

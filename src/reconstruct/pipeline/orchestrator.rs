use std::collections::{HashMap, HashSet};
use std::io::BufWriter;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use rayon::prelude::*;

use crate::bundle;
use crate::compress::decompress_data;
use crate::hash::{ChunkHash, hash_hex};
use crate::manifest::{ArtifactStorageMode, CompressionProfile, snapshot::SnapshotManifest};
use crate::path_guard::resolve_manifest_relative_path;
use crate::progress::GutchOperationControl;
use crate::repo::{GutchRepoPaths, chunk_path};
use crate::runtime::{
    RuntimePlan, RuntimeResourceHints, plan_reconstruct_runtime, run_in_thread_pool,
};
use crate::{Error, Result};

use super::super::types::{
    BundleAccessPlan, BundleAccessStrategy, ChunkLocation, LoadedChunk, ReconstructOptions,
    ReconstructTimings, typed_chunk_refs,
};
use super::direct::{
    direct_reconstruct_bundle_compressed_to_files, should_use_direct_bundle_to_file_reconstruct,
};
use super::staging::stage_reconstruct_bundle_access;

// ---------------------------------------------------------------------------
// Entry preparation
// ---------------------------------------------------------------------------

pub(in crate::reconstruct) fn prepare_reconstruct(
    snapshot_manifest: &SnapshotManifest,
    output_dir: &Path,
    runtime_hints: &RuntimeResourceHints,
    control: &GutchOperationControl,
) -> Result<(HashSet<ChunkHash>, Instant, RuntimePlan, ReconstructTimings)> {
    control.checkpoint()?;
    crate::manifest::validate_snapshot_entries(&snapshot_manifest.entries)?;
    let total_start = Instant::now();
    let needed_chunks = crate::repo::required_chunk_hashes_typed(snapshot_manifest)?;
    let runtime_plan = plan_reconstruct_runtime(runtime_hints);
    let mut timings = ReconstructTimings {
        runtime_plan: Some(runtime_plan.clone()),
        ..ReconstructTimings::default()
    };
    crate::helpers::create_dir_all(output_dir)?;
    create_snapshot_directories(snapshot_manifest, output_dir)?;
    timings.prepare_wall = total_start.elapsed();
    Ok((needed_chunks, total_start, runtime_plan, timings))
}

pub(in crate::reconstruct) fn create_snapshot_directories(
    snapshot_manifest: &SnapshotManifest,
    output_dir: &Path,
) -> Result<()> {
    let mut directories = snapshot_manifest
        .entries
        .iter()
        .filter_map(|entry| match entry {
            crate::manifest::SnapshotEntry::Directory(directory) => Some(directory),
            _ => None,
        })
        .collect::<Vec<_>>();
    directories.sort_by_key(|directory| directory.path.matches('/').count());
    for directory in directories {
        let path = resolve_manifest_relative_path(output_dir, &directory.path)?;
        crate::helpers::create_dir_all(&path)?;
    }
    Ok(())
}

fn relative_link_text(link_path: &str, target_path: &str) -> PathBuf {
    let parent = Path::new(link_path)
        .parent()
        .unwrap_or_else(|| Path::new(""));
    let parent_parts = parent.components().collect::<Vec<_>>();
    let target_parts = Path::new(target_path).components().collect::<Vec<_>>();
    let mut common = 0usize;
    while common < parent_parts.len()
        && common < target_parts.len()
        && parent_parts[common] == target_parts[common]
    {
        common += 1;
    }
    let mut relative = PathBuf::new();
    for _ in common..parent_parts.len() {
        relative.push("..");
    }
    for component in &target_parts[common..] {
        relative.push(component.as_os_str());
    }
    relative
}

pub(in crate::reconstruct) fn finalize_snapshot_entries(
    snapshot_manifest: &SnapshotManifest,
    output_dir: &Path,
) -> Result<()> {
    for entry in &snapshot_manifest.entries {
        let crate::manifest::SnapshotEntry::Symlink(link) = entry else {
            continue;
        };
        let link_path = resolve_manifest_relative_path(output_dir, &link.path)?;
        let link_text = relative_link_text(&link.path, &link.target_path);
        #[cfg(unix)]
        std::os::unix::fs::symlink(&link_text, &link_path).map_err(|error| {
            Error::Reconstruct(format!(
                "failed to create symbolic link '{}': {error}",
                link.path
            ))
        })?;
        #[cfg(windows)]
        {
            let target_is_directory = snapshot_manifest.entries.iter().any(|target| {
                target.path() == link.target_path
                    && matches!(target, crate::manifest::SnapshotEntry::Directory(_))
            });
            let result = if target_is_directory {
                std::os::windows::fs::symlink_dir(&link_text, &link_path)
            } else {
                std::os::windows::fs::symlink_file(&link_text, &link_path)
            };
            result.map_err(|error| Error::Reconstruct(format!(
                "failed to create symbolic link '{}'; enable Windows Developer Mode or grant symbolic-link privilege: {error}",
                link.path
            )))?;
        }
    }

    for file in snapshot_manifest.files() {
        let path = resolve_manifest_relative_path(output_dir, &file.path)?;
        #[cfg(unix)]
        if file.executable {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&path)?.permissions();
            permissions.set_mode(permissions.mode() | 0o111);
            std::fs::set_permissions(&path, permissions)?;
        }
        let mut permissions = std::fs::metadata(&path)?.permissions();
        permissions.set_readonly(file.read_only);
        std::fs::set_permissions(&path, permissions)?;
    }

    let mut directories = snapshot_manifest
        .entries
        .iter()
        .filter_map(|entry| match entry {
            crate::manifest::SnapshotEntry::Directory(directory) => Some(directory),
            _ => None,
        })
        .collect::<Vec<_>>();
    directories.sort_by_key(|directory| std::cmp::Reverse(directory.path.matches('/').count()));
    for directory in directories {
        let path = resolve_manifest_relative_path(output_dir, &directory.path)?;
        let mut permissions = std::fs::metadata(&path)?.permissions();
        permissions.set_readonly(directory.read_only);
        std::fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Main orchestrator — bundle-path based reconstruction
// ---------------------------------------------------------------------------

pub(in crate::reconstruct) struct BundleReconstructRequest<'a> {
    pub snapshot_manifest: &'a SnapshotManifest,
    pub bundle_paths: &'a [std::path::PathBuf],
    pub compression_profile: &'a CompressionProfile,
    pub needed_chunks: &'a HashSet<ChunkHash>,
    pub local_chunks: HashMap<ChunkHash, Vec<u8>>,
    pub output_dir: &'a Path,
    pub runtime_plan: RuntimePlan,
    pub total_start: Instant,
    pub timings: ReconstructTimings,
    pub options: ReconstructOptions,
    pub control: &'a GutchOperationControl,
}

pub(in crate::reconstruct) fn reconstruct_from_bundle_paths_and_local_chunks(
    request: BundleReconstructRequest<'_>,
) -> Result<((), ReconstructTimings)> {
    request.control.checkpoint()?;
    let mut timings = request.timings;
    timings.verify_chunks = request.options.verify_chunks;
    timings.verify_files = request.options.verify_files;
    let missing_chunks: HashSet<ChunkHash> = request
        .needed_chunks
        .iter()
        .filter(|chunk_hash| !request.local_chunks.contains_key(*chunk_hash))
        .copied()
        .collect();
    let mut locations: HashMap<ChunkHash, ChunkLocation> =
        HashMap::with_capacity(request.needed_chunks.len());
    let mut bundle_scan_info: HashMap<std::path::PathBuf, super::super::types::BundleScanInfo> =
        HashMap::new();

    for bundle_path in request.bundle_paths {
        request.control.checkpoint()?;
        let file = std::fs::File::open(bundle_path)?;
        let reader = bundle::BundleReader::open(file, request.compression_profile)?;
        bundle_scan_info.insert(
            bundle_path.clone(),
            super::super::types::BundleScanInfo {
                compression_unit: reader.compression_unit(),
                raw_payload_size: estimate_bundle_raw_payload_size(reader.entries()),
            },
        );
        for (index, entry) in reader.entries().iter().enumerate() {
            request.control.checkpoint()?;
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

    if should_use_direct_bundle_to_file_reconstruct(
        &request.local_chunks,
        &bundle_scan_info,
        &request.runtime_plan,
        request.options,
    ) {
        direct_reconstruct_bundle_compressed_to_files(super::direct::DirectReconstructRequest {
            snapshot_manifest: request.snapshot_manifest,
            locations: &locations,
            output_dir: request.output_dir,
            compression_profile: request.compression_profile,
            timings: &mut timings,
            worker_count: request.runtime_plan.worker_count,
            options: request.options,
            control: request.control,
        })?;
        finalize_snapshot_entries(request.snapshot_manifest, request.output_dir)?;
        timings.total = request.total_start.elapsed();
        return Ok(((), timings));
    }

    let access_plan = stage_reconstruct_bundle_access(
        &locations,
        &bundle_scan_info,
        &request.runtime_plan,
        request.compression_profile,
        &mut timings,
        request.control,
    )?;

    reconstruct_files_from_locations_and_chunks(
        ReconstructFilesRequest {
            snapshot_manifest: request.snapshot_manifest,
            locations: &locations,
            access_plan: &access_plan,
            local_chunks: &request.local_chunks,
            compression_profile: request.compression_profile,
            output_dir: request.output_dir,
            worker_count: request.runtime_plan.worker_count,
            options: request.options,
            control: request.control,
        },
        &mut timings,
    )?;
    finalize_snapshot_entries(request.snapshot_manifest, request.output_dir)?;
    timings.total = request.total_start.elapsed();
    Ok(((), timings))
}

// ---------------------------------------------------------------------------
// Loose chunk reading (repo-based reconstruction)
// ---------------------------------------------------------------------------

pub(in crate::reconstruct) struct ReadLooseRepoChunksRequest<'a> {
    pub repo_paths: &'a GutchRepoPaths,
    pub chunk_index: &'a crate::manifest::ChunkIndex,
    pub compression_profile: &'a CompressionProfile,
    pub needed_chunks: &'a HashSet<ChunkHash>,
    pub options: ReconstructOptions,
    pub worker_count: usize,
    pub timings: &'a mut ReconstructTimings,
    pub control: &'a GutchOperationControl,
}

pub(in crate::reconstruct) fn read_loose_repo_chunks(
    request: ReadLooseRepoChunksRequest<'_>,
) -> Result<HashMap<ChunkHash, Vec<u8>>> {
    let typed_chunk_index_entries = crate::repo::chunk_index_entries_typed(request.chunk_index)?;
    let loaded_chunks = run_in_thread_pool(request.worker_count, || {
        request
            .needed_chunks
            .par_iter()
            .filter_map(|chunk_hash| {
                if let Err(error) = request.control.checkpoint() {
                    return Some(Err(error));
                }
                let index_entry = typed_chunk_index_entries.get(chunk_hash)?;
                if index_entry.storage_mode != ArtifactStorageMode::LooseChunks {
                    return None;
                }
                Some(read_loose_repo_chunk(
                    request.repo_paths,
                    request.compression_profile,
                    chunk_hash,
                    request.options,
                ))
            })
            .collect::<Vec<_>>()
    })?;

    let mut chunk_map = HashMap::with_capacity(loaded_chunks.len());
    for loaded_chunk in loaded_chunks {
        request.control.checkpoint()?;
        let loaded_chunk = loaded_chunk?;
        request.timings.bundle_read += loaded_chunk.read_duration;
        request.timings.bundle_read_compressed_bytes += loaded_chunk.compressed_bytes;
        request.timings.bundle_read_decompressed_bytes += loaded_chunk.decompressed_bytes;
        request.timings.chunk_verify += loaded_chunk.chunk_verify_duration;
        if request.options.verify_chunks {
            request.timings.chunk_verify_bytes += loaded_chunk.decompressed_bytes;
        }
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
    let path = chunk_path(repo_paths, &chunk_hash_hex)?;
    let read_start = Instant::now();
    let stored = crate::helpers::read(&path)?;
    let read_duration = read_start.elapsed();
    let chunk_data = decompress_data(&stored, compression_profile.compression)?;
    let verify_started_at = Instant::now();
    verify_chunk_hash(&chunk_data, *chunk_hash, options)?;
    let chunk_verify_duration = if options.verify_chunks {
        verify_started_at.elapsed()
    } else {
        std::time::Duration::ZERO
    };
    Ok(LoadedChunk {
        chunk_hash: *chunk_hash,
        compressed_bytes: stored.len() as u64,
        decompressed_bytes: chunk_data.len() as u64,
        data: chunk_data,
        read_duration,
        chunk_verify_duration,
    })
}

// ---------------------------------------------------------------------------
// File reconstruction from chunk map (in-memory bundles path)
// ---------------------------------------------------------------------------

struct ReconstructFilesRequest<'a> {
    snapshot_manifest: &'a SnapshotManifest,
    locations: &'a HashMap<ChunkHash, ChunkLocation>,
    access_plan: &'a BundleAccessPlan,
    local_chunks: &'a HashMap<ChunkHash, Vec<u8>>,
    compression_profile: &'a CompressionProfile,
    output_dir: &'a Path,
    worker_count: usize,
    options: ReconstructOptions,
    control: &'a GutchOperationControl,
}

pub(in crate::reconstruct) fn rebuild_files_from_chunks(
    snapshot_manifest: &SnapshotManifest,
    chunk_map: &HashMap<ChunkHash, Vec<u8>>,
    output_dir: &Path,
    timings: &mut ReconstructTimings,
    control: &GutchOperationControl,
) -> Result<()> {
    for fm in snapshot_manifest.files() {
        control.checkpoint()?;
        let mut file_data = Vec::with_capacity(fm.size as usize);

        for chunk_ref in typed_chunk_refs(fm)? {
            control.checkpoint()?;
            let chunk_hash = chunk_ref.hash;
            let chunk_data = chunk_map
                .get(&chunk_hash)
                .ok_or_else(|| Error::MissingChunk(chunk_ref.hash_hex.to_string()))?;

            file_data.extend_from_slice(chunk_data);
        }
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

    finalize_snapshot_entries(snapshot_manifest, output_dir)
}

// ---------------------------------------------------------------------------
// Per-file reconstruction (staged/direct access paths)
// ---------------------------------------------------------------------------

fn reconstruct_files_from_locations_and_chunks(
    request: ReconstructFilesRequest<'_>,
    timings: &mut ReconstructTimings,
) -> Result<()> {
    let per_file = run_in_thread_pool(request.worker_count, || {
        request
            .snapshot_manifest
            .entries
            .par_iter()
            .filter_map(crate::manifest::SnapshotEntry::as_file)
            .map(|fm| {
                request.control.checkpoint()?;
                reconstruct_manifest_file(ReconstructFileRequest {
                    fm,
                    locations: request.locations,
                    access_plan: request.access_plan,
                    local_chunks: request.local_chunks,
                    compression_profile: request.compression_profile,
                    output_dir: request.output_dir,
                    options: request.options,
                    control: request.control,
                })
            })
            .collect::<Vec<_>>()
    })?;

    for result in per_file {
        request.control.checkpoint()?;
        let file_timings = result?;
        timings.accumulate(&file_timings);
    }

    Ok(())
}

struct ReconstructFileRequest<'a> {
    fm: &'a crate::manifest::FileManifest,
    locations: &'a HashMap<ChunkHash, ChunkLocation>,
    access_plan: &'a BundleAccessPlan,
    local_chunks: &'a HashMap<ChunkHash, Vec<u8>>,
    compression_profile: &'a CompressionProfile,
    output_dir: &'a Path,
    options: ReconstructOptions,
    control: &'a GutchOperationControl,
}

fn reconstruct_manifest_file(request: ReconstructFileRequest<'_>) -> Result<ReconstructTimings> {
    request.control.checkpoint()?;
    let mut timings = ReconstructTimings::default();
    let output_path = resolve_manifest_relative_path(request.output_dir, &request.fm.path)?;
    if let Some(parent) = output_path.parent() {
        crate::helpers::create_dir_all(parent)?;
    }

    let mut output = BufWriter::new(std::fs::File::create(&output_path)?);
    let mut hasher = request.options.verify_files.then(blake3::Hasher::new);
    let mut written = 0u64;
    let mut staged_chunk_buffer = Vec::new();
    let mut bundle_readers: HashMap<std::path::PathBuf, bundle::BundleReader<std::fs::File>> =
        HashMap::new();
    let mut staged_raw_readers: HashMap<std::path::PathBuf, std::fs::File> = HashMap::new();

    for chunk_ref in typed_chunk_refs(request.fm)? {
        request.control.checkpoint()?;
        let chunk_hash = chunk_ref.hash;
        if let Some(local_chunk) = request.local_chunks.get(&chunk_hash) {
            write_reconstructed_chunk(
                &mut output,
                local_chunk,
                &mut hasher,
                &mut timings,
                &mut written,
            )?;
            continue;
        }

        let location = request
            .locations
            .get(&chunk_hash)
            .ok_or_else(|| Error::MissingChunk(chunk_ref.hash_hex.to_string()))?;
        let strategy = request
            .access_plan
            .strategies
            .get(&location.bundle_path)
            .ok_or_else(|| Error::MissingChunk(chunk_ref.hash_hex.to_string()))?;
        if let BundleAccessStrategy::StagedBundleMemory { raw_payload } = strategy {
            let chunk_data = read_staged_memory_chunk_for_reconstruct(
                raw_payload,
                location,
                chunk_hash,
                &mut timings,
                request.options,
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
                        entry.insert(bundle::BundleReader::open(
                            file,
                            request.compression_profile,
                        )?)
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
                verify_chunk_hash_with_timings(
                    &staged_chunk_buffer,
                    chunk_hash,
                    request.options,
                    &mut timings,
                )?;
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

    if written != request.fm.size {
        return Err(Error::Reconstruct(format!(
            "reconstructed size {} does not match expected {} for {}",
            written, request.fm.size, request.fm.path
        )));
    }

    let t = Instant::now();
    output.flush()?;
    timings.file_write += t.elapsed();

    if let Some(hasher) = hasher {
        let t = Instant::now();
        let actual_hash = hash_hex(&hasher.finalize());
        timings.file_verify += t.elapsed();
        if actual_hash != request.fm.full_file_hash {
            return Err(Error::HashMismatch {
                expected: request.fm.full_file_hash.clone(),
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

    let t = Instant::now();
    let chunk_data = slice_bundle_raw_payload(raw_payload, location, "staged")?;
    timings.bundle_read += t.elapsed();
    timings.bundle_read_decompressed_bytes += location.raw_size;
    verify_chunk_hash_with_timings(chunk_data, expected_chunk_hash, options, timings)?;
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

pub(in crate::reconstruct) fn verify_chunk_hash_with_timings(
    chunk_data: &[u8],
    expected_chunk_hash: ChunkHash,
    options: ReconstructOptions,
    timings: &mut ReconstructTimings,
) -> Result<()> {
    if !options.verify_chunks {
        return Ok(());
    }
    let started_at = Instant::now();
    verify_chunk_hash(chunk_data, expected_chunk_hash, options)?;
    timings.chunk_verify += started_at.elapsed();
    timings.chunk_verify_bytes += chunk_data.len() as u64;
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
        let verify_started_at = Instant::now();
        hasher.update(chunk_data);
        timings.file_verify += verify_started_at.elapsed();
        timings.file_verify_bytes += chunk_data.len() as u64;
    }
    *written += chunk_data.len() as u64;
    Ok(())
}

fn estimate_bundle_raw_payload_size(entries: &[crate::manifest::BundleTableEntry]) -> u64 {
    entries
        .iter()
        .map(|entry| entry.raw_offset.saturating_add(entry.raw_size))
        .max()
        .unwrap_or(0)
}

use std::collections::HashMap;
use std::collections::HashSet;
use std::io::Write;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rayon::prelude::*;
use tempfile::TempDir;

use crate::bundle;
use crate::hash::hash_hex;
use crate::manifest::target::TargetManifest;
use crate::path_guard::resolve_manifest_relative_path;
use crate::repo::required_chunk_hashes;
use crate::runtime::{
    RuntimePlan, RuntimeResourceHints, install_in_thread_pool, plan_reconstruct_runtime,
};
use crate::{Error, Result};

#[derive(Debug, Clone, Default)]
pub struct ReconstructTimings {
    pub runtime_plan: Option<RuntimePlan>,
    pub bundle_read: Duration,
    pub bundle_read_compressed_bytes: u64,
    pub bundle_read_decompressed_bytes: u64,
    pub file_rebuild: Duration,
    pub file_rebuild_bytes: u64,
    pub file_verify: Duration,
    pub file_verify_bytes: u64,
    pub file_write: Duration,
    pub file_write_bytes: u64,
    pub total: Duration,
}

#[derive(Clone)]
struct ChunkLocation {
    bundle_path: PathBuf,
    index: usize,
    compressed_size: u64,
    raw_offset: u64,
    raw_size: u64,
}

#[derive(Clone)]
struct BundleScanInfo {
    compression_unit: String,
}

#[derive(Clone)]
enum BundleAccessStrategy {
    DirectChunkRead,
    StagedBundleRaw { staged_raw_path: PathBuf },
}

struct BundleAccessPlan {
    strategies: HashMap<PathBuf, BundleAccessStrategy>,
    _staging_dir: Option<TempDir>,
}

pub struct VerifiedSourceInstall<'a> {
    pub target_manifest: &'a TargetManifest,
    pub install_dir: &'a Path,
}

pub fn reconstruct_from_target_manifest(
    target_manifest: &TargetManifest,
    bundles: &[Vec<u8>],
    output_dir: &Path,
) -> Result<()> {
    let (_, _) =
        reconstruct_from_target_manifest_with_timings(target_manifest, bundles, output_dir)?;
    Ok(())
}

pub fn reconstruct_from_target_manifest_with_timings(
    target_manifest: &TargetManifest,
    bundles: &[Vec<u8>],
    output_dir: &Path,
) -> Result<((), ReconstructTimings)> {
    let total_start = Instant::now();
    let mut timings = ReconstructTimings::default();
    std::fs::create_dir_all(output_dir)?;

    let needed_chunks = required_chunk_hashes(target_manifest);

    let mut chunk_map: HashMap<String, Vec<u8>> = HashMap::with_capacity(needed_chunks.len());

    for bundle_data in bundles {
        let t = Instant::now();
        let mut reader = bundle::read_bundle(bundle_data)?;
        let matching_indices: Vec<usize> = reader
            .entries()
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                if needed_chunks.contains(&entry.chunk_hash)
                    && !chunk_map.contains_key(&entry.chunk_hash)
                {
                    Some(index)
                } else {
                    None
                }
            })
            .collect();
        if matching_indices.is_empty() {
            continue;
        }

        timings.bundle_read_compressed_bytes += bundle_data.len() as u64;
        for index in matching_indices {
            let hash = reader.entries()[index].chunk_hash.clone();
            let data = reader.read_chunk(index)?;
            timings.bundle_read_decompressed_bytes += data.len() as u64;
            chunk_map.insert(hash, data);
        }
        timings.bundle_read += t.elapsed();

        if chunk_map.len() == needed_chunks.len() {
            break;
        }
    }

    rebuild_files_from_chunks(target_manifest, &chunk_map, output_dir, &mut timings)?;
    timings.total = total_start.elapsed();
    Ok(((), timings))
}

pub fn reconstruct_from_bundle_paths_with_timings(
    target_manifest: &TargetManifest,
    bundle_paths: &[PathBuf],
    output_dir: &Path,
) -> Result<((), ReconstructTimings)> {
    reconstruct_from_bundle_paths_with_timings_and_hints(
        target_manifest,
        bundle_paths,
        output_dir,
        &RuntimeResourceHints::default(),
    )
}

pub fn reconstruct_from_bundle_paths_with_timings_and_hints(
    target_manifest: &TargetManifest,
    bundle_paths: &[PathBuf],
    output_dir: &Path,
    runtime_hints: &RuntimeResourceHints,
) -> Result<((), ReconstructTimings)> {
    reconstruct_from_bundle_paths_and_verified_source_with_timings_and_hints(
        target_manifest,
        bundle_paths,
        None,
        output_dir,
        runtime_hints,
    )
}

pub fn reconstruct_from_bundle_paths_and_verified_source_with_timings(
    target_manifest: &TargetManifest,
    bundle_paths: &[PathBuf],
    verified_source: Option<VerifiedSourceInstall<'_>>,
    output_dir: &Path,
) -> Result<((), ReconstructTimings)> {
    reconstruct_from_bundle_paths_and_verified_source_with_timings_and_hints(
        target_manifest,
        bundle_paths,
        verified_source,
        output_dir,
        &RuntimeResourceHints::default(),
    )
}

pub fn reconstruct_from_bundle_paths_and_verified_source_with_timings_and_hints(
    target_manifest: &TargetManifest,
    bundle_paths: &[PathBuf],
    verified_source: Option<VerifiedSourceInstall<'_>>,
    output_dir: &Path,
    runtime_hints: &RuntimeResourceHints,
) -> Result<((), ReconstructTimings)> {
    let total_start = Instant::now();
    let runtime_plan = plan_reconstruct_runtime(runtime_hints);
    let mut timings = ReconstructTimings {
        runtime_plan: Some(runtime_plan.clone()),
        ..ReconstructTimings::default()
    };
    std::fs::create_dir_all(output_dir)?;

    let needed_chunks = required_chunk_hashes(target_manifest);

    let local_chunks = match verified_source {
        Some(source) => read_reusable_source_chunks(source, &needed_chunks)?,
        None => HashMap::new(),
    };
    let missing_chunks: HashSet<String> = needed_chunks
        .iter()
        .filter(|chunk_hash| !local_chunks.contains_key(*chunk_hash))
        .cloned()
        .collect();
    let mut locations: HashMap<String, ChunkLocation> = HashMap::with_capacity(needed_chunks.len());
    let mut bundle_scan_info: HashMap<PathBuf, BundleScanInfo> = HashMap::new();

    for bundle_path in bundle_paths {
        let file = std::fs::File::open(bundle_path)?;
        let reader = bundle::BundleReader::open(file)?;
        bundle_scan_info.insert(
            bundle_path.clone(),
            BundleScanInfo {
                compression_unit: reader.compression_unit().to_string(),
            },
        );
        for (index, entry) in reader.entries().iter().enumerate() {
            if missing_chunks.contains(&entry.chunk_hash) {
                locations
                    .entry(entry.chunk_hash.clone())
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
    let access_plan = stage_reconstruct_bundle_access(
        &locations,
        &bundle_scan_info,
        runtime_plan.worker_count,
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
    )?;
    timings.total = total_start.elapsed();
    Ok(((), timings))
}

fn read_reusable_source_chunks(
    source: VerifiedSourceInstall<'_>,
    needed_chunks: &HashSet<String>,
) -> Result<HashMap<String, Vec<u8>>> {
    let mut chunk_map: HashMap<String, Vec<u8>> = HashMap::new();

    for fm in &source.target_manifest.files {
        if fm
            .chunks
            .iter()
            .all(|chunk_ref| !needed_chunks.contains(&chunk_ref.chunk_hash))
        {
            continue;
        }

        let file_path = resolve_manifest_relative_path(source.install_dir, &fm.path)?;
        let file = std::fs::File::open(&file_path)?;
        let mut reader = std::io::BufReader::new(file);
        let mut chunk_buffer = Vec::new();

        let mut current_offset = 0u64;
        for chunk_ref in &fm.chunks {
            let chunk_size = chunk_ref.raw_size as usize;
            if !needed_chunks.contains(&chunk_ref.chunk_hash)
                || chunk_map.contains_key(&chunk_ref.chunk_hash)
            {
                chunk_buffer.resize(chunk_size, 0);
                if let Err(e) = reader.read_exact(&mut chunk_buffer) {
                    if e.kind() == std::io::ErrorKind::UnexpectedEof {
                        return Err(Error::Reconstruct(format!(
                            "source chunk out of bounds in {} at offset {}",
                            fm.path, current_offset
                        )));
                    }
                    return Err(e.into());
                }
                current_offset += chunk_ref.raw_size;
                continue;
            }

            chunk_buffer.resize(chunk_size, 0);
            if let Err(e) = reader.read_exact(&mut chunk_buffer) {
                if e.kind() == std::io::ErrorKind::UnexpectedEof {
                    return Err(Error::Reconstruct(format!(
                        "source chunk out of bounds in {} at offset {}",
                        fm.path, current_offset
                    )));
                }
                return Err(e.into());
            }

            let actual_hash = hash_hex(&crate::hash::hash_chunk(&chunk_buffer));
            if actual_hash != chunk_ref.chunk_hash {
                return Err(Error::HashMismatch {
                    expected: chunk_ref.chunk_hash.clone(),
                    actual: actual_hash,
                });
            }

            chunk_map.insert(chunk_ref.chunk_hash.clone(), chunk_buffer.clone());
            current_offset += chunk_ref.raw_size;
        }
    }

    Ok(chunk_map)
}

fn reconstruct_files_from_locations_and_chunks(
    target_manifest: &TargetManifest,
    locations: &HashMap<String, ChunkLocation>,
    access_plan: &BundleAccessPlan,
    local_chunks: &HashMap<String, Vec<u8>>,
    output_dir: &Path,
    timings: &mut ReconstructTimings,
    worker_count: usize,
) -> Result<()> {
    let per_file = install_in_thread_pool(worker_count, || {
        target_manifest
            .files
            .par_iter()
            .map(|fm| {
                reconstruct_manifest_file(fm, locations, access_plan, local_chunks, output_dir)
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
    locations: &HashMap<String, ChunkLocation>,
    access_plan: &BundleAccessPlan,
    local_chunks: &HashMap<String, Vec<u8>>,
    output_dir: &Path,
) -> Result<ReconstructTimings> {
    let mut timings = ReconstructTimings::default();
    let output_path = resolve_manifest_relative_path(output_dir, &fm.path)?;
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut output = std::fs::File::create(&output_path)?;
    let mut hasher = blake3::Hasher::new();
    let mut written = 0u64;
    let mut bundle_readers: HashMap<PathBuf, bundle::BundleReader<std::fs::File>> = HashMap::new();
    let mut staged_raw_readers: HashMap<PathBuf, std::fs::File> = HashMap::new();

    for chunk_ref in &fm.chunks {
        if let Some(local_chunk) = local_chunks.get(&chunk_ref.chunk_hash) {
            let t = Instant::now();
            output.write_all(local_chunk)?;
            timings.file_write += t.elapsed();
            timings.file_write_bytes += local_chunk.len() as u64;
            hasher.update(local_chunk);
            written += local_chunk.len() as u64;
            timings.file_rebuild_bytes += local_chunk.len() as u64;
            continue;
        }

        let location = locations
            .get(&chunk_ref.chunk_hash)
            .ok_or_else(|| Error::MissingChunk(chunk_ref.chunk_hash.clone()))?;
        let strategy = access_plan
            .strategies
            .get(&location.bundle_path)
            .ok_or_else(|| Error::MissingChunk(chunk_ref.chunk_hash.clone()))?;
        let chunk_data = read_chunk_for_reconstruct(
            strategy,
            location,
            &chunk_ref.chunk_hash,
            &mut bundle_readers,
            &mut staged_raw_readers,
            &mut timings,
        )?;

        let t = Instant::now();
        output.write_all(&chunk_data)?;
        timings.file_write += t.elapsed();
        timings.file_write_bytes += chunk_data.len() as u64;
        hasher.update(&chunk_data);
        written += chunk_data.len() as u64;
        timings.file_rebuild_bytes += chunk_data.len() as u64;
    }

    if written != fm.size {
        return Err(Error::Reconstruct(format!(
            "reconstructed size {} does not match expected {} for {}",
            written, fm.size, fm.path
        )));
    }

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

    Ok(timings)
}

fn read_chunk_for_reconstruct(
    strategy: &BundleAccessStrategy,
    location: &ChunkLocation,
    expected_chunk_hash: &str,
    bundle_readers: &mut HashMap<PathBuf, bundle::BundleReader<std::fs::File>>,
    staged_raw_readers: &mut HashMap<PathBuf, std::fs::File>,
    timings: &mut ReconstructTimings,
) -> Result<Vec<u8>> {
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
            let chunk_data = reader.read_chunk(location.index)?;
            timings.bundle_read += t.elapsed();
            timings.bundle_read_compressed_bytes += location.compressed_size;
            timings.bundle_read_decompressed_bytes += chunk_data.len() as u64;
            Ok(chunk_data)
        }
        BundleAccessStrategy::StagedBundleRaw { staged_raw_path } => {
            let staged_file = match staged_raw_readers.entry(staged_raw_path.clone()) {
                std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(std::fs::File::open(staged_raw_path)?)
                }
            };

            let mut chunk_data = vec![0u8; location.raw_size as usize];
            let t = Instant::now();
            staged_file.seek(SeekFrom::Start(location.raw_offset))?;
            staged_file.read_exact(&mut chunk_data)?;
            timings.bundle_read += t.elapsed();
            timings.bundle_read_decompressed_bytes += location.raw_size;

            let actual_hash = hash_hex(&crate::hash::hash_chunk(&chunk_data));
            if actual_hash != expected_chunk_hash {
                return Err(Error::HashMismatch {
                    expected: expected_chunk_hash.to_string(),
                    actual: actual_hash,
                });
            }

            Ok(chunk_data)
        }
    }
}

fn stage_reconstruct_bundle_access(
    locations: &HashMap<String, ChunkLocation>,
    bundle_scan_info: &HashMap<PathBuf, BundleScanInfo>,
    worker_count: usize,
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
            Error::Reconstruct(format!(
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

    let staging_dir = tempfile::tempdir().map_err(|err| {
        Error::Reconstruct(format!("failed to create bundle staging dir: {}", err))
    })?;
    let staged_results = install_in_thread_pool(worker_count, || {
        bundle_compressed_paths
            .par_iter()
            .enumerate()
            .map(|(index, bundle_path)| {
                stage_bundle_raw_payload(bundle_path, staging_dir.path(), index)
            })
            .collect::<Vec<_>>()
    })?;

    for staged in staged_results {
        let (bundle_path, staged_raw_path, compressed_size, staged_raw_size, stage_time) = staged?;
        timings.bundle_read += stage_time;
        timings.bundle_read_compressed_bytes += compressed_size;
        timings.bundle_read_decompressed_bytes += staged_raw_size;
        strategies.insert(
            bundle_path,
            BundleAccessStrategy::StagedBundleRaw { staged_raw_path },
        );
    }

    Ok(BundleAccessPlan {
        strategies,
        _staging_dir: Some(staging_dir),
    })
}

fn stage_bundle_raw_payload(
    bundle_path: &Path,
    staging_root: &Path,
    index: usize,
) -> Result<(PathBuf, PathBuf, u64, u64, Duration)> {
    let stage_start = Instant::now();
    let file = std::fs::File::open(bundle_path)?;
    let mut reader = bundle::BundleReader::open(file)?;
    let bundle_stem = bundle_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("bundle");
    let staged_raw_path = staging_root.join(format!("{}-{}.raw", index, bundle_stem));
    let mut staged_file = std::fs::File::create(&staged_raw_path)?;
    let staged_raw_size = reader.stage_bundle_payload_to(&mut staged_file)?;
    Ok((
        bundle_path.to_path_buf(),
        staged_raw_path,
        reader.payload_size(),
        staged_raw_size,
        stage_start.elapsed(),
    ))
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

fn rebuild_files_from_chunks(
    target_manifest: &TargetManifest,
    chunk_map: &HashMap<String, Vec<u8>>,
    output_dir: &Path,
    timings: &mut ReconstructTimings,
) -> Result<()> {
    for fm in &target_manifest.files {
        let t = Instant::now();
        let mut file_data = Vec::with_capacity(fm.size as usize);

        for chunk_ref in &fm.chunks {
            let chunk_data = chunk_map
                .get(&chunk_ref.chunk_hash)
                .ok_or_else(|| Error::MissingChunk(chunk_ref.chunk_hash.clone()))?;

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
            std::fs::create_dir_all(parent)?;
        }
        let t = Instant::now();
        std::fs::write(&output_path, &file_data)?;
        timings.file_write += t.elapsed();
        timings.file_write_bytes += file_data.len() as u64;
    }

    Ok(())
}

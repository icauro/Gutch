pub mod bundle;
pub mod chunk;
pub mod compress;
pub mod error;
pub mod hash;
pub mod manifest;
#[cfg(test)]
mod test_util;

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use bundle::BundleBuilder;
use manifest::{ChunkRef, FileManifest, TargetManifest};
use rayon::prelude::*;

pub use error::{Error, Result};

use crate::compress::compress_chunk;
use crate::hash::hash_hex;

const MAX_BUNDLE_SIZE: u64 = 64 * 1024 * 1024;
const MIN_PREFERRED_BUNDLE_SIZE: u64 = 8 * 1024 * 1024;

fn file_mode(metadata: &std::fs::Metadata) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        return metadata.permissions().mode();
    }
    #[cfg(not(unix))]
    {
        if metadata.permissions().readonly() {
            0o444
        } else {
            0o644
        }
    }
}

pub struct BuildResult {
    pub target_manifest: TargetManifest,
    pub bundles: Vec<BuiltBundle>,
}

#[derive(Debug, Clone)]
pub struct BuiltBundle {
    pub bundle_id: String,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct ExistingChunkLocation {
    pub bundle_id: String,
    pub raw_size: u64,
    pub compressed_size: u64,
}

#[derive(Debug, Clone, Default)]
pub struct BuildTimings {
    pub scan_read: Duration,
    pub scan_read_bytes: u64,
    pub file_hash: Duration,
    pub file_hash_bytes: u64,
    pub chunking: Duration,
    pub chunking_bytes: u64,
    pub chunk_hash_judgment: Duration,
    pub chunk_hash_judgment_bytes: u64,
    pub metadata: Duration,
    pub dedupe_lookup: Duration,
    pub dedupe_lookup_bytes: u64,
    pub compression: Duration,
    pub compression_input_bytes: u64,
    pub compression_output_bytes: u64,
    pub total_compressed_bytes: u64,
    pub bundle_build: Duration,
    pub bundle_build_bytes: u64,
    pub target_manifest_finalize: Duration,
    pub total: Duration,
}

#[derive(Debug, Clone, Default)]
pub struct ReconstructTimings {
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

#[derive(Debug, Clone, Default)]
pub struct VerifyTimings {
    pub stat: Duration,
    pub file_read: Duration,
    pub file_read_bytes: u64,
    pub file_hash: Duration,
    pub file_hash_bytes: u64,
    pub chunk_hash: Duration,
    pub chunk_hash_bytes: u64,
    pub total: Duration,
}

struct FileBuildEntry {
    manifest: FileManifest,
    chunks: Vec<(String, Vec<u8>)>,
    size: u64,
    scan_read: Duration,
    file_hash: Duration,
    chunking: Duration,
    chunking_bytes: u64,
    chunk_hash_judgment: Duration,
    chunk_hash_judgment_bytes: u64,
    metadata: Duration,
}

#[derive(Clone)]
struct CompressedChunk {
    hash: String,
    raw_size: u64,
    compressed: Vec<u8>,
}

#[derive(Clone)]
struct ChunkLocation {
    bundle_path: PathBuf,
    index: usize,
    compressed_size: u64,
}

#[derive(Debug, Clone, Copy)]
struct BundleSizingPolicy {
    max_size: u64,
    min_preferred_size: u64,
}

impl Default for BundleSizingPolicy {
    fn default() -> Self {
        Self {
            max_size: MAX_BUNDLE_SIZE,
            min_preferred_size: MIN_PREFERRED_BUNDLE_SIZE,
        }
    }
}

impl BundleSizingPolicy {
    fn target_size_for(&self, compressed_bytes: u64) -> u64 {
        if compressed_bytes == 0 {
            return self.max_size;
        }

        if compressed_bytes <= self.max_size {
            return compressed_bytes.max(self.min_preferred_size).min(self.max_size);
        }

        let bundle_count = compressed_bytes.div_ceil(self.max_size);
        compressed_bytes
            .div_ceil(bundle_count)
            .max(self.min_preferred_size)
            .min(self.max_size)
    }
}

pub fn build_from_directory(
    dir: &Path,
    product_id: &str,
    version_id: &str,
    platform: &str,
    architecture: &str,
    tier_id: &str,
) -> Result<BuildResult> {
    let (result, _) = build_from_directory_with_timings_against_existing_bundles(
        dir,
        product_id,
        version_id,
        platform,
        architecture,
        tier_id,
        &HashSet::new(),
    )?;
    Ok(result)
}

pub fn build_from_directory_with_timings(
    dir: &Path,
    product_id: &str,
    version_id: &str,
    platform: &str,
    architecture: &str,
    tier_id: &str,
) -> Result<(BuildResult, BuildTimings)> {
    build_from_directory_with_timings_against_existing_bundles(
        dir,
        product_id,
        version_id,
        platform,
        architecture,
        tier_id,
        &HashSet::new(),
    )
}

pub fn build_from_directory_with_timings_against_existing_bundles(
    dir: &Path,
    product_id: &str,
    version_id: &str,
    platform: &str,
    architecture: &str,
    tier_id: &str,
    existing_bundle_ids: &HashSet<String>,
) -> Result<(BuildResult, BuildTimings)> {
    build_from_directory_with_timings_against_existing_store(
        dir,
        product_id,
        version_id,
        platform,
        architecture,
        tier_id,
        &HashMap::new(),
        existing_bundle_ids,
    )
}

pub fn build_from_directory_with_timings_against_existing_store(
    dir: &Path,
    product_id: &str,
    version_id: &str,
    platform: &str,
    architecture: &str,
    tier_id: &str,
    existing_chunk_locations: &HashMap<String, ExistingChunkLocation>,
    existing_bundle_ids: &HashSet<String>,
) -> Result<(BuildResult, BuildTimings)> {
    build_from_directory_with_timings_against_existing_store_and_manifests(
        dir,
        product_id,
        version_id,
        platform,
        architecture,
        tier_id,
        existing_chunk_locations,
        existing_bundle_ids,
        &HashMap::new(),
    )
}

pub fn build_from_directory_with_timings_against_existing_store_and_manifests(
    dir: &Path,
    product_id: &str,
    version_id: &str,
    platform: &str,
    architecture: &str,
    tier_id: &str,
    existing_chunk_locations: &HashMap<String, ExistingChunkLocation>,
    existing_bundle_ids: &HashSet<String>,
    previous_file_manifests: &HashMap<String, FileManifest>,
) -> Result<(BuildResult, BuildTimings)> {
    let total_start = Instant::now();
    let chunking_profile = manifest::ChunkingProfile::default();
    let compression_profile = manifest::CompressionProfile::default();
    let mut timings = BuildTimings::default();

    let mut file_paths: Vec<PathBuf> = Vec::new();
    for entry in walkdir::WalkDir::new(dir).sort_by_file_name() {
        let entry = entry?;
        if entry.path().is_file() {
            file_paths.push(entry.path().to_path_buf());
        }
    }

    let file_entries: Vec<FileBuildEntry> = file_paths
        .par_iter()
        .map(|path| -> Result<FileBuildEntry> {
            let relative = path.strip_prefix(dir).map_err(|e| Error::PathError {
                path: path.to_path_buf(),
                reason: e.to_string(),
            })?;

            let t = Instant::now();
            let file_data = std::fs::read(path)?;
            let scan_read = t.elapsed();
            let file_size = file_data.len() as u64;

            let t = Instant::now();
            let full_file_hash = hash_hex(&hash::hash_chunk(&file_data));
            let file_hash = t.elapsed();

            if let Some(previous) = previous_file_manifests.get(&relative.to_string_lossy().replace('\\', "/")) {
                if previous.size == file_size && previous.full_file_hash == full_file_hash {
                    return Ok(FileBuildEntry {
                        manifest: previous.clone(),
                        chunks: Vec::new(),
                        size: file_size,
                        scan_read,
                        file_hash,
                        chunking: Duration::ZERO,
                        chunking_bytes: 0,
                        chunk_hash_judgment: Duration::ZERO,
                        chunk_hash_judgment_bytes: 0,
                        metadata: Duration::ZERO,
                    });
                }
            }

            let (chunk_results, chunk_timings) = chunk::chunk_slices(&file_data, &chunking_profile);
            let chunking = chunk_timings.boundary_scan;

            let mut chunk_refs = Vec::with_capacity(chunk_results.len());
            let mut chunks = Vec::with_capacity(chunk_results.len());
            for cr in &chunk_results {
                let ch = hash_hex(&cr.hash);
                chunk_refs.push(ChunkRef {
                    chunk_hash: ch.clone(),
                    raw_size: cr.len as u64,
                    file_offset: cr.offset,
                });
                if !existing_chunk_locations.contains_key(&ch) {
                    let start = cr.offset as usize;
                    chunks.push((ch, file_data[start..start + cr.len].to_vec()));
                }
            }

            let t = Instant::now();
            let metadata = std::fs::metadata(path)?;
            let mode = file_mode(&metadata);
            let metadata = t.elapsed();

            Ok(FileBuildEntry {
                manifest: FileManifest {
                    path: relative.to_string_lossy().replace('\\', "/"),
                    size: file_size,
                    mode,
                    executable: false,
                    symlink_target: None,
                    full_file_hash,
                    chunks: chunk_refs,
                },
                chunks,
                size: file_size,
                scan_read,
                file_hash,
                chunking,
                chunking_bytes: chunk_timings.boundary_scan_bytes,
                chunk_hash_judgment: chunk_timings.hash_judgment,
                chunk_hash_judgment_bytes: chunk_timings.hash_judgment_bytes,
                metadata,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    let mut file_manifests: Vec<FileManifest> = Vec::with_capacity(file_entries.len());
    let mut unique_chunks: HashMap<String, Vec<u8>> = HashMap::new();
    let mut total_logical_size: u64 = 0;

    for entry in file_entries {
        timings.scan_read += entry.scan_read;
        timings.scan_read_bytes += entry.size;
        timings.file_hash += entry.file_hash;
        timings.file_hash_bytes += entry.size;
        timings.chunking += entry.chunking;
        timings.chunking_bytes += entry.chunking_bytes;
        timings.chunk_hash_judgment += entry.chunk_hash_judgment;
        timings.chunk_hash_judgment_bytes += entry.chunk_hash_judgment_bytes;
        timings.metadata += entry.metadata;
        total_logical_size += entry.size;
        file_manifests.push(entry.manifest);
        for (hash, data) in entry.chunks {
            unique_chunks.entry(hash).or_insert(data);
        }
    }

    let mut all_chunk_hashes: HashSet<String> = HashSet::new();
    let mut required_bundle_set = Vec::new();
    let mut total_compressed: u64 = 0;
    let mut compressed_estimate_seen: HashSet<String> = HashSet::new();
    for fm in &file_manifests {
        for chunk_ref in &fm.chunks {
            all_chunk_hashes.insert(chunk_ref.chunk_hash.clone());
            if let Some(existing) = existing_chunk_locations.get(&chunk_ref.chunk_hash) {
                required_bundle_set.push(existing.bundle_id.clone());
                if compressed_estimate_seen.insert(chunk_ref.chunk_hash.clone()) {
                    total_compressed += existing.compressed_size;
                }
            }
        }
    }

    let mut chunks_to_compress = Vec::new();

    let t = Instant::now();
    for (hash, data) in unique_chunks {
        let raw_size = data.len() as u64;
        timings.dedupe_lookup_bytes += raw_size;

        if let Some(existing) = existing_chunk_locations.get(&hash) {
            if compressed_estimate_seen.insert(hash.clone()) {
                total_compressed += existing.compressed_size;
            }
        } else {
            chunks_to_compress.push((hash, data));
        }
    }
    timings.dedupe_lookup += t.elapsed();

    for (_, data) in &chunks_to_compress {
        timings.compression_input_bytes += data.len() as u64;
    }

    let t = Instant::now();
    let compressed_chunks: Vec<CompressedChunk> = chunks_to_compress
        .into_par_iter()
        .map(|(hash, data)| -> Result<CompressedChunk> {
            let raw_size = data.len() as u64;
            let compressed = compress_chunk(&data, &compression_profile)?;
            Ok(CompressedChunk { hash, raw_size, compressed })
        })
        .collect::<Result<Vec<_>>>()?;
    timings.compression += t.elapsed();

    for chunk in &compressed_chunks {
        timings.compression_output_bytes += chunk.compressed.len() as u64;
        if compressed_estimate_seen.insert(chunk.hash.clone()) {
            total_compressed += chunk.compressed.len() as u64;
        }
    }
    timings.total_compressed_bytes = total_compressed;

    let mut sorted_chunks = compressed_chunks;
    sorted_chunks.sort_by(|a, b| a.hash.cmp(&b.hash));

    let t = Instant::now();
    let mut bundles = Vec::new();
    let mut builder = BundleBuilder::new(compression_profile.clone());
    let mut current_bundle_compressed_size: u64 = 0;
    let new_compressed_bytes: u64 = sorted_chunks.iter().map(|c| c.compressed.len() as u64).sum();
    let bundle_target_size = BundleSizingPolicy::default().target_size_for(new_compressed_bytes);

    for chunk in sorted_chunks {
        let compressed_size = chunk.compressed.len() as u64;
        if !builder.is_empty() && current_bundle_compressed_size + compressed_size > bundle_target_size {
            let bundle_id = builder.compute_bundle_id();
            required_bundle_set.push(bundle_id.clone());
            if !existing_bundle_ids.contains(&bundle_id) {
                let bundle_data = builder.build()?;
                bundles.push(BuiltBundle { bundle_id, data: bundle_data });
            }
            builder = BundleBuilder::new(compression_profile.clone());
            current_bundle_compressed_size = 0;
        }

        builder.add_precompressed_chunk(chunk.hash, chunk.raw_size, chunk.compressed);
        current_bundle_compressed_size += compressed_size;
    }

    if !builder.is_empty() {
        let bundle_id = builder.compute_bundle_id();
        required_bundle_set.push(bundle_id.clone());
        if !existing_bundle_ids.contains(&bundle_id) {
            let bundle_data = builder.build()?;
            bundles.push(BuiltBundle { bundle_id, data: bundle_data });
        }
    }

    timings.bundle_build += t.elapsed();
    timings.bundle_build_bytes = bundles.iter().map(|b| b.data.len() as u64).sum();

    let t = Instant::now();
    file_manifests.sort_by(|a, b| a.path.cmp(&b.path));
    let mut vm = TargetManifest::new(
        product_id.to_string(),
        version_id.to_string(),
        platform.to_string(),
        architecture.to_string(),
        tier_id.to_string(),
    );
    vm.total_logical_size = total_logical_size;
    vm.total_compressed_size_estimate = total_compressed;
    vm.files = file_manifests;
    vm.required_chunk_set = all_chunk_hashes.into_iter().collect();
    vm.required_chunk_set.sort();
    required_bundle_set.sort();
    required_bundle_set.dedup();
    vm.required_bundle_set = required_bundle_set;
    timings.target_manifest_finalize += t.elapsed();
    timings.total = total_start.elapsed();

    Ok((BuildResult {
        target_manifest: vm,
        bundles,
    }, timings))
}

pub fn reconstruct_from_target_manifest(
    target_manifest: &TargetManifest,
    bundles: &[Vec<u8>],
    output_dir: &Path,
) -> Result<()> {
    let (_, _) = reconstruct_from_target_manifest_with_timings(target_manifest, bundles, output_dir)?;
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

    let needed_chunks: HashSet<String> = target_manifest
        .files
        .iter()
        .flat_map(|fm| fm.chunks.iter().map(|cr| cr.chunk_hash.clone()))
        .collect();

    let mut chunk_map: HashMap<String, Vec<u8>> = HashMap::with_capacity(needed_chunks.len());

    for bundle_data in bundles {
        let t = Instant::now();
        let mut reader = bundle::read_bundle(bundle_data)?;
        let matching_indices: Vec<usize> = reader
            .entries()
            .iter()
            .enumerate()
            .filter_map(|e| {
                let (index, entry) = e;
                if needed_chunks.contains(&entry.chunk_hash) && !chunk_map.contains_key(&entry.chunk_hash) {
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
    let total_start = Instant::now();
    let mut timings = ReconstructTimings::default();
    std::fs::create_dir_all(output_dir)?;

    let needed_chunks: HashSet<String> = target_manifest
        .files
        .iter()
        .flat_map(|fm| fm.chunks.iter().map(|cr| cr.chunk_hash.clone()))
        .collect();

    let mut locations: HashMap<String, ChunkLocation> = HashMap::with_capacity(needed_chunks.len());

    for bundle_path in bundle_paths {
        let file = std::fs::File::open(bundle_path)?;
        let reader = bundle::BundleReader::open(file)?;
        for (index, entry) in reader.entries().iter().enumerate() {
            if needed_chunks.contains(&entry.chunk_hash) {
                locations.entry(entry.chunk_hash.clone()).or_insert_with(|| ChunkLocation {
                    bundle_path: bundle_path.clone(),
                    index,
                    compressed_size: entry.compressed_size,
                });
            }
        }

        if locations.len() == needed_chunks.len() {
            break;
        }
    }

    reconstruct_files_from_locations(target_manifest, &locations, output_dir, &mut timings)?;
    timings.total = total_start.elapsed();
    Ok(((), timings))
}

fn reconstruct_files_from_locations(
    target_manifest: &TargetManifest,
    locations: &HashMap<String, ChunkLocation>,
    output_dir: &Path,
    timings: &mut ReconstructTimings,
) -> Result<()> {
    let mut readers: HashMap<PathBuf, bundle::BundleReader<std::fs::File>> = HashMap::new();

    for fm in &target_manifest.files {
        let output_path = output_dir.join(&fm.path);
        if let Some(parent) = output_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut output = std::fs::File::create(&output_path)?;
        let mut hasher = blake3::Hasher::new();
        let mut written = 0u64;

        for chunk_ref in &fm.chunks {
            let location = locations
                .get(&chunk_ref.chunk_hash)
                .ok_or_else(|| Error::MissingChunk(chunk_ref.chunk_hash.clone()))?;

            let reader = match readers.entry(location.bundle_path.clone()) {
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

            let target_offset = chunk_ref.file_offset;
            if target_offset > written {
                let gap = vec![0u8; (target_offset - written) as usize];
                let t = Instant::now();
                output.write_all(&gap)?;
                timings.file_write += t.elapsed();
                timings.file_write_bytes += gap.len() as u64;
                hasher.update(&gap);
                written += gap.len() as u64;
            }

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
                written,
                fm.size,
                fm.path
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
    }

    Ok(())
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

            let target_offset = chunk_ref.file_offset as usize;
            if target_offset > file_data.len() {
                file_data.resize(target_offset, 0);
            }

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
        let actual_hash = hash_hex(&hash::hash_chunk(&file_data));
        timings.file_verify += t.elapsed();
        timings.file_verify_bytes += file_data.len() as u64;
        if actual_hash != fm.full_file_hash {
            return Err(Error::HashMismatch {
                expected: fm.full_file_hash.clone(),
                actual: actual_hash,
            });
        }

        let output_path = output_dir.join(&fm.path);
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

pub fn verify_directory(
    target_manifest: &TargetManifest,
    dir: &Path,
) -> Result<Vec<String>> {
    let (errors, _) = verify_directory_with_timings(target_manifest, dir)?;
    Ok(errors)
}

pub fn verify_directory_with_timings(
    target_manifest: &TargetManifest,
    dir: &Path,
) -> Result<(Vec<String>, VerifyTimings)> {
    let total_start = Instant::now();
    let mut errors = Vec::new();
    let mut timings = VerifyTimings::default();

    for fm in &target_manifest.files {
        let file_path = dir.join(&fm.path);

        if !file_path.exists() {
            errors.push(format!("missing file: {}", fm.path));
            continue;
        }

        let t = Instant::now();
        let metadata = match std::fs::metadata(&file_path) {
            Ok(m) => m,
            Err(e) => {
                errors.push(format!("cannot stat {}: {}", fm.path, e));
                continue;
            }
        };
        timings.stat += t.elapsed();

        if metadata.len() != fm.size {
            errors.push(format!(
                "size mismatch for {}: expected {}, got {}",
                fm.path,
                fm.size,
                metadata.len()
            ));
            continue;
        }

        let t = Instant::now();
        let file_data = match std::fs::read(&file_path) {
            Ok(d) => d,
            Err(e) => {
                errors.push(format!("cannot read {}: {}", fm.path, e));
                continue;
            }
        };
        timings.file_read += t.elapsed();
        timings.file_read_bytes += file_data.len() as u64;

        let t = Instant::now();
        let actual_hash = hash_hex(&hash::hash_chunk(&file_data));
        timings.file_hash += t.elapsed();
        timings.file_hash_bytes += file_data.len() as u64;
        if actual_hash != fm.full_file_hash {
            errors.push(format!(
                "hash mismatch for {}: expected {}, got {}",
                fm.path,
                fm.full_file_hash,
                actual_hash
            ));
        }

        for chunk_ref in &fm.chunks {
            let start = chunk_ref.file_offset as usize;
            let end = start + chunk_ref.raw_size as usize;
            if end > file_data.len() {
                errors.push(format!(
                    "chunk reference out of bounds in {}: offset {}, size {}",
                    fm.path,
                    chunk_ref.file_offset,
                    chunk_ref.raw_size
                ));
                continue;
            }

            let t = Instant::now();
            let chunk_bytes = &file_data[start..end];
            let actual_chunk_hash = hash_hex(&hash::hash_chunk(chunk_bytes));
            timings.chunk_hash += t.elapsed();
            timings.chunk_hash_bytes += chunk_bytes.len() as u64;
            if actual_chunk_hash != chunk_ref.chunk_hash {
                errors.push(format!(
                    "chunk hash mismatch in {} at offset {}: expected {}, got {}",
                    fm.path,
                    chunk_ref.file_offset,
                    chunk_ref.chunk_hash,
                    actual_chunk_hash
                ));
            }
        }
    }

    timings.total = total_start.elapsed();
    Ok((errors, timings))
}

pub fn estimate_build(
    target_manifest: &TargetManifest,
    available_chunks: &HashSet<String>,
    available_disk_space: u64,
) -> Result<manifest::Estimate> {
    let mut missing_chunks: HashSet<String> = HashSet::new();
    let mut missing_logical: u64 = 0;

    for fm in &target_manifest.files {
        for cr in &fm.chunks {
            if !available_chunks.contains(&cr.chunk_hash) {
                missing_chunks.insert(cr.chunk_hash.clone());
                missing_logical += cr.raw_size;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util;
    use std::time::Instant;

    #[test]
    fn test_build_and_reconstruct_roundtrip() {
        let dir = std::env::temp_dir().join("gutch_test_roundtrip");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let sub = dir.join("subdir");
        std::fs::create_dir_all(&sub).unwrap();

        std::fs::write(dir.join("file1.txt"), b"hello world, this is file one ".repeat(50)).unwrap();
        std::fs::write(dir.join("file2.txt"), b"goodbye world, this is file two ".repeat(80)).unwrap();
        std::fs::write(sub.join("nested.txt"), b"nested content here ".repeat(30)).unwrap();

        let result = build_from_directory(
            &dir, "test-product", "1.0.0", "windows", "x86_64", "base",
        )
        .unwrap();

        assert!(!result.target_manifest.files.is_empty());
        assert!(!result.target_manifest.required_chunk_set.is_empty());

        let out_dir = std::env::temp_dir().join("gutch_test_output");
        let _ = std::fs::remove_dir_all(&out_dir);

        let bundle_data: Vec<Vec<u8>> = result.bundles.iter().map(|b| b.data.clone()).collect();
        reconstruct_from_target_manifest(&result.target_manifest, &bundle_data, &out_dir)
            .unwrap();

        let verify_errors = verify_directory(&result.target_manifest, &out_dir).unwrap();

        assert!(verify_errors.is_empty(), "verification errors: {:?}", verify_errors);

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&out_dir);
    }

    #[test]
    fn test_verify_directory_missing_file() {
        let dir = std::env::temp_dir().join("gutch_test_verify");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let fm = FileManifest {
            path: "nonexistent.txt".to_string(),
            size: 10,
            mode: 0o644,
            executable: false,
            symlink_target: None,
            full_file_hash: "abc".to_string(),
            chunks: vec![],
        };

        let vm = TargetManifest::new("p".into(), "v".into(), "win".into(), "x64".into(), "base".into());
        let mut vm = vm;
        vm.files.push(fm);
        let errors = verify_directory(&vm, &dir).unwrap();
        assert!(!errors.is_empty());
        assert!(errors[0].contains("missing"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_large_game_build_200mb() {
        let root = std::env::temp_dir().join("gutch_test_200mb");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        for sub in &["audio", "textures", "data"] {
            std::fs::create_dir_all(root.join(sub)).unwrap();
        }

        let t0 = Instant::now();

        test_util::make_file(root.join("game.exe"), 42 * 1024 * 1024, test_util::Seed::Binary);
        test_util::make_file(root.join("audio/eng.pak"), 48 * 1024 * 1024, test_util::Seed::Audio);
        test_util::make_file(root.join("audio/fr.pak"), 32 * 1024 * 1024, test_util::Seed::Audio);
        test_util::make_file(root.join("textures/characters.tex"), 22 * 1024 * 1024, test_util::Seed::Texture);
        test_util::make_file(root.join("textures/environment.tex"), 22 * 1024 * 1024, test_util::Seed::Texture);
        test_util::make_file(root.join("data/assets.pak"), 38 * 1024 * 1024, test_util::Seed::Archive);
        test_util::make_file(root.join("data/config.json"), 2 * 1024 * 1024, test_util::Seed::Config);
        test_util::make_file(root.join("readme.txt"), 500 * 1024, test_util::Seed::Text);

        let gen_ms = t0.elapsed().as_millis();

        let total_original = test_util::dir_size(&root);
        assert!(total_original >= 190 * 1024 * 1024, "expected >=190 MB, got {} MB", total_original / (1024 * 1024));

        let t1 = Instant::now();
        let result = build_from_directory(&root, "mock-game", "1.0.0", "windows", "x86_64", "base").unwrap();
        let build_ms = t1.elapsed().as_millis();

        let bundle_total: u64 = result.bundles.iter().map(|b| b.data.len() as u64).sum();
        let compression_pct = if result.target_manifest.total_logical_size > 0 {
            100.0 * bundle_total as f64 / result.target_manifest.total_logical_size as f64
        } else {
            0.0
        };

        let t2 = Instant::now();
        let out_dir = std::env::temp_dir().join("gutch_test_200mb_reconstructed");
        let _ = std::fs::remove_dir_all(&out_dir);
        let bundle_data: Vec<Vec<u8>> = result.bundles.iter().map(|b| b.data.clone()).collect();
        reconstruct_from_target_manifest(&result.target_manifest, &bundle_data, &out_dir).unwrap();
        let recon_ms = t2.elapsed().as_millis();

        let t3 = Instant::now();
        let errors = verify_directory(&result.target_manifest, &out_dir).unwrap();
        let verify_ms = t3.elapsed().as_millis();

        assert!(errors.is_empty(), "verification errors: {:?}", errors);

        let est = estimate_build(&result.target_manifest, &Default::default(), 1024 * 1024 * 1024).unwrap();

        println!();
        println!("=== 200 MB mock game build ===");
        println!("  files:           {}", result.target_manifest.files.len());
        println!("  total logical:   {:.1} MB", result.target_manifest.total_logical_size as f64 / (1024.0 * 1024.0));
        println!("  total bundled:   {:.1} MB ({:.1}% of original)", bundle_total as f64 / (1024.0 * 1024.0), compression_pct);
        println!("  unique chunks:   {}", result.target_manifest.required_chunk_set.len());
        println!("  bundles:         {}", result.bundles.len());
        println!();
        println!("  gen files:       {} ms", gen_ms);
        println!("  build:           {} ms", build_ms);
        println!("  reconstruct:     {} ms", recon_ms);
        println!("  verify:          {} ms", verify_ms);
        println!("  total pipeline:  {} ms", gen_ms + build_ms + recon_ms + verify_ms);
        println!();
        println!("  fresh install estimate:");
        println!("    missing chunks:      {}", est.missing_chunk_count);
        println!("    missing logical:     {:.1} MB", est.missing_logical_bytes as f64 / (1024.0 * 1024.0));
        println!("    missing compressed:  {:.1} MB", est.missing_compressed_bytes as f64 / (1024.0 * 1024.0));
        println!("    estimated bundles:   {}", est.required_bundle_count);

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&out_dir);
    }
}

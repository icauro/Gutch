use std::collections::HashMap;
use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::bundle;
use crate::hash::hash_hex;
use crate::manifest::TargetManifest;
use crate::{Error, Result};

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

#[derive(Clone)]
struct ChunkLocation {
    bundle_path: PathBuf,
    index: usize,
    compressed_size: u64,
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
                locations
                    .entry(entry.chunk_hash.clone())
                    .or_insert_with(|| ChunkLocation {
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
        let actual_hash = hash_hex(&crate::hash::hash_chunk(&file_data));
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

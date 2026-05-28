use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rayon::prelude::*;

use crate::build_types::{BuildTimings, ExistingChunkLocation};
use crate::hash::hash_hex;
use crate::manifest::{self, ChunkRef, FileManifest};
use crate::{Error, Result};

pub struct FileBuildEntry {
    pub manifest: FileManifest,
    pub chunks: Vec<(String, Vec<u8>)>,
    pub size: u64,
    pub scan_read: Duration,
    pub file_hash: Duration,
    pub chunking: Duration,
    pub chunking_bytes: u64,
    pub chunk_hash_judgment: Duration,
    pub chunk_hash_judgment_bytes: u64,
    pub metadata: Duration,
}

pub(crate) fn accumulate_entry_timings(timings: &mut BuildTimings, entry: &FileBuildEntry) {
    timings.scan_read += entry.scan_read;
    timings.scan_read_bytes += entry.size;
    timings.file_hash += entry.file_hash;
    timings.file_hash_bytes += entry.size;
    timings.chunking += entry.chunking;
    timings.chunking_bytes += entry.chunking_bytes;
    timings.chunk_hash_judgment += entry.chunk_hash_judgment;
    timings.chunk_hash_judgment_bytes += entry.chunk_hash_judgment_bytes;
    timings.metadata += entry.metadata;
}

pub(crate) fn collect_scanned_chunks(
    dir: &Path,
    previous_file_manifests: &HashMap<String, FileManifest>,
    chunking_profile: &manifest::ChunkSizingProfile,
    existing_chunk_locations: &HashMap<String, ExistingChunkLocation>,
    timings: &mut BuildTimings,
) -> Result<(Vec<FileManifest>, HashMap<String, Vec<u8>>, u64)> {
    let file_entries = scan_directory_into_file_entries(
        dir,
        previous_file_manifests,
        chunking_profile,
        existing_chunk_locations,
    )?;

    let mut file_manifests = Vec::with_capacity(file_entries.len());
    let mut unique_chunks = HashMap::new();
    let mut total_logical_size: u64 = 0;

    for entry in file_entries {
        accumulate_entry_timings(timings, &entry);
        total_logical_size += entry.size;
        file_manifests.push(entry.manifest);
        for (hash, data) in entry.chunks {
            unique_chunks.entry(hash).or_insert(data);
        }
    }

    Ok((file_manifests, unique_chunks, total_logical_size))
}

pub(crate) fn scan_directory_into_file_entries(
    dir: &Path,
    previous_file_manifests: &HashMap<String, FileManifest>,
    chunking_profile: &manifest::ChunkSizingProfile,
    existing_chunk_locations: &HashMap<String, ExistingChunkLocation>,
) -> Result<Vec<FileBuildEntry>> {
    let mut file_paths: Vec<PathBuf> = Vec::new();
    for entry in walkdir::WalkDir::new(dir).sort_by_file_name() {
        let entry = entry?;
        if entry.path().is_file() {
            file_paths.push(entry.path().to_path_buf());
        }
    }

    file_paths
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
            let full_file_hash = hash_hex(&crate::hash::hash_chunk(&file_data));
            let file_hash = t.elapsed();

            if let Some(previous) =
                previous_file_manifests.get(&relative.to_string_lossy().replace('\\', "/"))
            {
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

            let (chunk_results, chunk_timings) =
                crate::chunk::chunk_slices(&file_data, chunking_profile);
            let chunking = chunk_timings.boundary_scan;

            let mut chunk_refs = Vec::with_capacity(chunk_results.len());
            let mut chunks = Vec::with_capacity(chunk_results.len());
            for cr in &chunk_results {
                let ch = hash_hex(&cr.hash);
                chunk_refs.push(ChunkRef {
                    chunk_hash: ch.clone(),
                    raw_size: cr.len as u64,
                });
                if !existing_chunk_locations.contains_key(&ch) {
                    let start = cr.offset as usize;
                    chunks.push((ch, file_data[start..start + cr.len].to_vec()));
                }
            }

            let t = std::time::Instant::now();
            let _fs_meta = std::fs::metadata(path)?;
            let metadata_dur = t.elapsed();

            Ok(FileBuildEntry {
                manifest: FileManifest {
                    path: relative.to_string_lossy().replace('\\', "/"),
                    size: file_size,
                    executable: false,
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
                metadata: metadata_dur,
            })
        })
        .collect::<Result<Vec<_>>>()
}

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use rayon::prelude::*;

use crate::build_types::{BuildTimings, ChunkPayload, ExistingChunkLocation};
use crate::hash::hash_hex;
use crate::manifest::{self, ChunkRef, FileManifest};
use crate::runtime::{ChunkStorageMode, install_in_thread_pool};
use crate::{Error, Result};

pub struct FileBuildEntry {
    pub manifest: FileManifest,
    pub chunks: Vec<(String, ChunkPayload)>,
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
    chunk_storage_mode: ChunkStorageMode,
    chunk_spool_root: Option<&Path>,
    worker_count: usize,
    timings: &mut BuildTimings,
) -> Result<(Vec<FileManifest>, HashMap<String, ChunkPayload>, u64)> {
    let file_entries = scan_directory_into_file_entries(
        dir,
        previous_file_manifests,
        chunking_profile,
        existing_chunk_locations,
        chunk_storage_mode,
        chunk_spool_root,
        worker_count,
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
    chunk_storage_mode: ChunkStorageMode,
    chunk_spool_root: Option<&Path>,
    worker_count: usize,
) -> Result<Vec<FileBuildEntry>> {
    let mut file_paths: Vec<PathBuf> = Vec::new();
    for entry in walkdir::WalkDir::new(dir).sort_by_file_name() {
        let entry = entry?;
        if entry.path().is_file() {
            file_paths.push(entry.path().to_path_buf());
        }
    }

    let spool_root = chunk_spool_root.map(PathBuf::from);
    let spill_counter = Arc::new(AtomicU64::new(0));
    let scan_one = |path: &PathBuf| -> Result<FileBuildEntry> {
            let relative = path.strip_prefix(dir).map_err(|e| Error::PathError {
                path: path.to_path_buf(),
                reason: e.to_string(),
            })?;

            let scan_read = Duration::ZERO;
            let (chunk_results, chunk_timings) =
                crate::chunk::chunk_file_streaming(path, chunking_profile)?;
            let file_size: u64 = chunk_results.iter().map(|chunk| chunk.data.len() as u64).sum();

            let t = Instant::now();
            let mut full_file_hasher = blake3::Hasher::new();
            for chunk in &chunk_results {
                full_file_hasher.update(&chunk.data);
            }
            let full_file_hash = hash_hex(&full_file_hasher.finalize());
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

            let chunking = chunk_timings.boundary_scan;

            let mut chunk_refs = Vec::with_capacity(chunk_results.len());
            let mut chunks = Vec::with_capacity(chunk_results.len());
            for cr in chunk_results {
                let ch = hash_hex(&cr.hash);
                chunk_refs.push(ChunkRef {
                    chunk_hash: ch.clone(),
                    raw_size: cr.data.len() as u64,
                });
                if !existing_chunk_locations.contains_key(&ch) {
                    let payload = match chunk_storage_mode {
                        ChunkStorageMode::Memory => ChunkPayload::InMemory { bytes: cr.data },
                        ChunkStorageMode::TempSpill => {
                            let spool_root = spool_root.as_ref().ok_or_else(|| {
                                Error::manifest(
                                    "temp spill mode requires a chunk spool directory",
                                )
                            })?;
                            let chunk_id = spill_counter.fetch_add(1, Ordering::Relaxed);
                            let chunk_path = spool_root.join(format!("chunk-{chunk_id:016x}.bin"));
                            std::fs::write(&chunk_path, &cr.data)?;
                            ChunkPayload::TempFile {
                                path: chunk_path,
                                raw_size: cr.data.len() as u64,
                            }
                        }
                    };
                    chunks.push((ch, payload));
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
        };

    if worker_count <= 1 {
        file_paths.iter().map(scan_one).collect::<Result<Vec<_>>>()
    } else {
        install_in_thread_pool(worker_count, || {
            file_paths
                .par_iter()
                .map(scan_one)
                .collect::<Result<Vec<_>>>()
        })?
    }
}

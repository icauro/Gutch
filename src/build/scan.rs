use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rayon::prelude::*;

use crate::build::types::{BuildProgressSink, BuildTimings, ChunkPayload, ExistingChunkLocation};
use crate::hash::ChunkHash;
use crate::manifest::{self, ChunkRef, FileManifest};
use crate::runtime::{ChunkStorageMode, install_in_thread_pool};
use crate::{Error, Result};

pub struct FileBuildEntry {
    pub manifest: FileManifest,
    pub chunks: Vec<(ChunkHash, ChunkPayload)>,
    pub size: u64,
    pub scan_read: Duration,
    pub file_hash: Duration,
    pub chunking: Duration,
    pub chunking_bytes: u64,
    pub chunk_hash_judgment: Duration,
    pub chunk_hash_judgment_bytes: u64,
    pub metadata: Duration,
}

pub(crate) struct ScanDirectoryOptions<'a> {
    pub previous_file_manifests: &'a HashMap<String, FileManifest>,
    pub chunking_profile: &'a manifest::ChunkSizingProfile,
    pub existing_chunk_locations: &'a HashMap<ChunkHash, ExistingChunkLocation>,
    pub chunk_storage_mode: ChunkStorageMode,
    pub chunk_spool_root: Option<&'a Path>,
    pub worker_count: usize,
    pub progress: Option<Arc<dyn BuildProgressSink>>,
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
    options: ScanDirectoryOptions<'_>,
    timings: &mut BuildTimings,
) -> Result<(Vec<FileManifest>, HashMap<ChunkHash, ChunkPayload>, u64)> {
    let file_entries = scan_directory_into_file_entries(dir, options)?;

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
    options: ScanDirectoryOptions<'_>,
) -> Result<Vec<FileBuildEntry>> {
    let mut file_paths: Vec<PathBuf> = Vec::new();
    for entry in walkdir::WalkDir::new(dir).sort_by_file_name() {
        let entry = entry?;
        if entry.path().is_file() {
            file_paths.push(entry.path().to_path_buf());
        }
    }

    if let Some(progress) = &options.progress {
        progress.on_file_scan_started(file_paths.len());
    }

    let spool_root = options.chunk_spool_root.map(PathBuf::from);
    let progress_for_scan = options.progress.clone();
    let scan_one = |path: &PathBuf| -> Result<FileBuildEntry> {
        let result = (|| -> Result<FileBuildEntry> {
            let relative = path.strip_prefix(dir).map_err(|e| Error::PathError {
                path: path.to_path_buf(),
                reason: e.to_string(),
            })?;
            let normalized_relative = relative.to_string_lossy().replace('\\', "/");

            let scan_read = Duration::ZERO;
            let (chunk_scan, chunk_timings) =
                crate::chunk::chunk_file_streaming_with_file_hash(path, options.chunking_profile)?;
            let file_size = chunk_scan.file_size;
            let full_file_hash = chunk_scan.full_file_hash;
            let file_hash = chunk_scan.full_file_hash_duration;

            if let Some(previous) = options.previous_file_manifests.get(&normalized_relative)
                && previous.size == file_size
                && previous.full_file_hash == full_file_hash
            {
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

            let chunking = chunk_timings.boundary_scan;

            let mut chunk_refs = Vec::with_capacity(chunk_scan.chunks.len());
            let mut chunks = Vec::with_capacity(chunk_scan.chunks.len());
            let mut spill_file = None;
            let mut spill_path = None;
            let mut spill_offset: u64 = 0;
            for cr in chunk_scan.chunks {
                let chunk_hash = ChunkHash::from_blake3(cr.hash);
                chunk_refs.push(ChunkRef {
                    chunk_hash: chunk_hash.to_hex(),
                    raw_size: cr.data.len() as u64,
                });
                if !options.existing_chunk_locations.contains_key(&chunk_hash) {
                    let payload = match options.chunk_storage_mode {
                        ChunkStorageMode::Memory => ChunkPayload::InMemory {
                            bytes: std::sync::Arc::<[u8]>::from(cr.data),
                        },
                        ChunkStorageMode::TempSpill => {
                            let spool_root = spool_root.as_ref().ok_or_else(|| {
                                Error::manifest("temp spill mode requires a chunk spool directory")
                            })?;
                            if spill_path.is_none() {
                                let relative_name =
                                    relative.to_string_lossy().replace(['\\', '/'], "__");
                                spill_path =
                                    Some(spool_root.join(format!("{relative_name}.spill")));
                            }
                            if spill_file.is_none() {
                                let path = spill_path.as_ref().ok_or_else(|| {
                                    Error::manifest("temp spill path missing after initialization")
                                })?;
                                spill_file = Some(std::fs::File::create(path)?);
                            }
                            let local_spill_path = spill_path.as_ref().ok_or_else(|| {
                                Error::manifest("temp spill path missing after file creation")
                            })?;
                            let file = spill_file.as_mut().ok_or_else(|| {
                                Error::manifest("temp spill file missing after creation")
                            })?;
                            let chunk_offset = spill_offset;
                            file.write_all(&cr.data)?;
                            spill_offset += cr.data.len() as u64;
                            ChunkPayload::TempFile {
                                path: local_spill_path.clone(),
                                offset: chunk_offset,
                                raw_size: cr.data.len() as u64,
                            }
                        }
                    };
                    chunks.push((chunk_hash, payload));
                }
            }

            let t = std::time::Instant::now();
            let _fs_meta = std::fs::metadata(path)?;
            let metadata_dur = t.elapsed();

            Ok(FileBuildEntry {
                manifest: FileManifest {
                    path: normalized_relative,
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
        })();

        if result.is_ok()
            && let Some(progress) = &progress_for_scan
        {
            progress.on_file_scanned();
        }

        result
    };

    if options.worker_count <= 1 {
        file_paths.iter().map(scan_one).collect::<Result<Vec<_>>>()
    } else {
        install_in_thread_pool(options.worker_count, || {
            file_paths
                .par_iter()
                .map(scan_one)
                .collect::<Result<Vec<_>>>()
        })?
    }
}

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
    pub prehash: Duration,
    pub prehash_bytes: u64,
    pub prehash_reused_bytes: u64,
    pub prehash_candidate_file_count: usize,
    pub prehash_reused_file_count: usize,
    pub prehash_mismatch_file_count: usize,
    pub file_hash: Duration,
    pub file_hash_bytes: u64,
    pub stream_chunk: Duration,
    pub stream_chunk_bytes: u64,
    pub chunk_hash_judgment: Duration,
    pub chunk_hash_judgment_bytes: u64,
    pub metadata: Duration,
}

pub(crate) struct ScanDirectoryOptions<'a> {
    pub previous_file_manifests: &'a HashMap<String, Vec<FileManifest>>,
    pub experimental_prehash_reuse: bool,
    pub chunking_profile: &'a manifest::ChunkSizingProfile,
    pub existing_chunk_locations: &'a HashMap<ChunkHash, ExistingChunkLocation>,
    pub chunk_storage_mode: ChunkStorageMode,
    pub chunk_spool_root: Option<&'a Path>,
    pub worker_count: usize,
    pub progress: Option<Arc<dyn BuildProgressSink>>,
    pub control: crate::GutchOperationControl,
}

pub(crate) fn accumulate_entry_timings(timings: &mut BuildTimings, entry: &FileBuildEntry) {
    timings.logical_bytes += entry.size;
    timings.prehash += entry.prehash;
    timings.prehash_bytes += entry.prehash_bytes;
    timings.prehash_reused_bytes += entry.prehash_reused_bytes;
    timings.prehash_candidate_file_count += entry.prehash_candidate_file_count;
    timings.prehash_reused_file_count += entry.prehash_reused_file_count;
    timings.prehash_mismatch_file_count += entry.prehash_mismatch_file_count;
    timings.file_hash += entry.file_hash;
    timings.file_hash_bytes += entry.file_hash_bytes;
    timings.stream_chunk += entry.stream_chunk;
    timings.stream_chunk_bytes += entry.stream_chunk_bytes;
    timings.chunk_hash_judgment += entry.chunk_hash_judgment;
    timings.chunk_hash_judgment_bytes += entry.chunk_hash_judgment_bytes;
    timings.metadata += entry.metadata;
}

pub(crate) fn collect_scanned_chunks(
    dir: &Path,
    options: ScanDirectoryOptions<'_>,
    timings: &mut BuildTimings,
) -> Result<(Vec<FileManifest>, HashMap<ChunkHash, ChunkPayload>, u64)> {
    let control = options.control.clone();
    let file_entries = scan_directory_into_file_entries(dir, options)?;

    let mut file_manifests = Vec::with_capacity(file_entries.len());
    let mut unique_chunks = HashMap::new();
    let mut total_logical_size: u64 = 0;

    for entry in file_entries {
        control.checkpoint()?;
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
        options.control.checkpoint()?;
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
    let control_for_scan = options.control.clone();
    let scan_one = |path: &PathBuf| -> Result<FileBuildEntry> {
        let result = (|| -> Result<FileBuildEntry> {
            control_for_scan.checkpoint()?;
            let relative = path.strip_prefix(dir).map_err(|e| Error::PathError {
                path: path.to_path_buf(),
                reason: e.to_string(),
            })?;
            let normalized_relative = relative.to_string_lossy().replace('\\', "/");

            let metadata_started_at = std::time::Instant::now();
            let file_metadata = std::fs::metadata(path)?;
            let metadata = metadata_started_at.elapsed();
            let previous_candidates = options
                .previous_file_manifests
                .get(&normalized_relative)
                .map(Vec::as_slice)
                .unwrap_or_default();

            if options.experimental_prehash_reuse
                && previous_candidates
                    .iter()
                    .any(|candidate| candidate.size == file_metadata.len())
            {
                let (prehash, prehash_bytes, prehash_duration) =
                    crate::hash::hash_file_streaming_with_control(path, &control_for_scan)?;
                let prehash_hex = crate::hash::hash_hex(&prehash);
                if let Some(previous) = previous_candidates.iter().find(|candidate| {
                    candidate.size == prehash_bytes && candidate.full_file_hash == prehash_hex
                }) {
                    return Ok(FileBuildEntry {
                        manifest: previous.clone(),
                        chunks: Vec::new(),
                        size: prehash_bytes,
                        prehash: prehash_duration,
                        prehash_bytes,
                        prehash_reused_bytes: prehash_bytes,
                        prehash_candidate_file_count: 1,
                        prehash_reused_file_count: 1,
                        prehash_mismatch_file_count: 0,
                        file_hash: Duration::ZERO,
                        file_hash_bytes: 0,
                        stream_chunk: Duration::ZERO,
                        stream_chunk_bytes: 0,
                        chunk_hash_judgment: Duration::ZERO,
                        chunk_hash_judgment_bytes: 0,
                        metadata,
                    });
                }

                return scan_changed_file(
                    path,
                    relative,
                    normalized_relative,
                    metadata,
                    prehash_duration,
                    prehash_bytes,
                    previous_candidates,
                    &options,
                    &control_for_scan,
                    spool_root.as_ref(),
                    true,
                );
            }

            scan_changed_file(
                path,
                relative,
                normalized_relative,
                metadata,
                Duration::ZERO,
                0,
                previous_candidates,
                &options,
                &control_for_scan,
                spool_root.as_ref(),
                false,
            )
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

#[allow(clippy::too_many_arguments)]
fn scan_changed_file(
    path: &Path,
    relative: &Path,
    normalized_relative: String,
    metadata: Duration,
    prehash: Duration,
    prehash_bytes: u64,
    previous_candidates: &[FileManifest],
    options: &ScanDirectoryOptions<'_>,
    control: &crate::GutchOperationControl,
    spool_root: Option<&PathBuf>,
    prehash_mismatch: bool,
) -> Result<FileBuildEntry> {
    let (chunk_scan, chunk_timings) =
        crate::chunk::chunk_file_streaming_with_file_hash_and_control(
            path,
            options.chunking_profile,
            control,
        )?;
    let file_size = chunk_scan.file_size;
    let full_file_hash = chunk_scan.full_file_hash;
    let file_hash = chunk_scan.full_file_hash_duration;

    if let Some(previous) = previous_candidates
        .iter()
        .find(|candidate| candidate.size == file_size && candidate.full_file_hash == full_file_hash)
    {
        return Ok(FileBuildEntry {
            manifest: previous.clone(),
            chunks: Vec::new(),
            size: file_size,
            prehash,
            prehash_bytes,
            prehash_reused_bytes: 0,
            prehash_candidate_file_count: usize::from(prehash_mismatch),
            prehash_reused_file_count: 0,
            prehash_mismatch_file_count: usize::from(prehash_mismatch),
            file_hash,
            file_hash_bytes: file_size,
            stream_chunk: chunk_timings.boundary_scan,
            stream_chunk_bytes: chunk_timings.boundary_scan_bytes,
            chunk_hash_judgment: chunk_timings.hash_judgment,
            chunk_hash_judgment_bytes: chunk_timings.hash_judgment_bytes,
            metadata,
        });
    }

    let stream_chunk = chunk_timings.boundary_scan;

    let mut chunk_refs = Vec::with_capacity(chunk_scan.chunks.len());
    let mut chunks = Vec::with_capacity(chunk_scan.chunks.len());
    let mut spill_file = None;
    let mut spill_path = None;
    let mut spill_offset: u64 = 0;
    for cr in chunk_scan.chunks {
        control.checkpoint()?;
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
                        let relative_name = relative.to_string_lossy().replace(['\\', '/'], "__");
                        spill_path = Some(spool_root.join(format!("{relative_name}.spill")));
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
                    let file = spill_file
                        .as_mut()
                        .ok_or_else(|| Error::manifest("temp spill file missing after creation"))?;
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
        prehash,
        prehash_bytes,
        prehash_reused_bytes: 0,
        prehash_candidate_file_count: usize::from(prehash_mismatch),
        prehash_reused_file_count: 0,
        prehash_mismatch_file_count: usize::from(prehash_mismatch),
        file_hash,
        file_hash_bytes: file_size,
        stream_chunk,
        stream_chunk_bytes: chunk_timings.boundary_scan_bytes,
        chunk_hash_judgment: chunk_timings.hash_judgment,
        chunk_hash_judgment_bytes: chunk_timings.hash_judgment_bytes,
        metadata,
    })
}

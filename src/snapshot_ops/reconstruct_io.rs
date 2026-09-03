use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::{
    ReconstructArtifactChunk, ReconstructArtifactJob, ReconstructFileTraceMetadata, ReconstructJob,
    ReconstructPipelineChunkInput, ReconstructPipelineFile, ReconstructPipelineReconstruction,
    ReconstructSourceChunkJob, ReconstructTarget, SnapshotReconstructSource, SourceChunkLocation,
};
use crate::progress::GutchOperationControl;
use crate::snapshot_ops::types::ReconstructTimings;
use crate::{Error, Result};

pub(super) fn reconstruct_job(
    job: &ReconstructJob,
    file_plans: &[ReconstructPipelineFile],
    output_paths: &[PathBuf],
    precomputed_file_hashes: &[Mutex<Option<String>>],
    output_handles: &OutputHandleCache,
    compression_profile: &crate::manifest::CompressionProfile,
    control: &GutchOperationControl,
) -> Result<ReconstructTimings> {
    match job {
        ReconstructJob::Artifact(job) => {
            reconstruct_artifact_job(job, output_handles, compression_profile, control)
        }
        ReconstructJob::SourceChunk(job) => {
            reconstruct_source_chunk_job(job, file_plans, output_handles, control)
        }
        ReconstructJob::CopySourceFile {
            file_index,
            source_path,
        } => copy_source_file_job(
            *file_index,
            source_path,
            file_plans,
            output_paths,
            precomputed_file_hashes,
            control,
        ),
    }
}

fn reconstruct_artifact_job(
    job: &ReconstructArtifactJob,
    output_handles: &OutputHandleCache,
    compression_profile: &crate::manifest::CompressionProfile,
    control: &GutchOperationControl,
) -> Result<ReconstructTimings> {
    control.checkpoint()?;
    let artifact_span_name = match &job.artifact {
        crate::ArtifactRef::Bundle { .. } => "snapshot.reconstruct.bundle",
        crate::ArtifactRef::LooseChunk { .. } => "snapshot.reconstruct.job.loose_chunk",
    };
    let _artifact_span =
        control.profile_span_with_args(artifact_span_name, reconstruct_artifact_trace_args(job));
    let profiler = control.profiler();
    let compressed_bytes = std::fs::metadata(&job.artifact_path)?.len();
    let mut timings = ReconstructTimings::default();
    let mut target_handles = HashMap::new();

    match &job.artifact {
        crate::ArtifactRef::Bundle { .. } => {
            let mut reader = crate::bundle::BundleReader::open(
                File::open(&job.artifact_path)?,
                compression_profile,
            )?;
            let mut required = vec![false; reader.entries().len()];
            let mut required_chunks = vec![None; reader.entries().len()];
            for (index, entry) in reader.entries().iter().enumerate() {
                if let Some(chunk) = job.chunks.get(&entry.chunk_hash).cloned() {
                    required[index] = true;
                    required_chunks[index] = Some((entry.clone(), chunk));
                }
            }
            let required_count = required.iter().filter(|required| **required).count();
            if required_count != job.chunks.len() {
                return Err(Error::reconstruct(format!(
                    "bundle '{}' is missing {} required chunks",
                    job.artifact.id(),
                    job.chunks.len().saturating_sub(required_count)
                )));
            }
            // Bundle jobs are parallelized by the outer reconstruction pool.
            // Each worker reads one bundle once and streams decoded chunks
            // directly into verification and positional fan-out. At most one
            // required chunk is retained while the next range is decoded.
            let decoded_payload_bytes = reader
                .entries()
                .iter()
                .try_fold(0u64, |total, entry| total.checked_add(entry.raw_size))
                .ok_or_else(|| Error::reconstruct("bundle raw size overflows"))?;
            let _fanout_span = profiler.map(|profiler| {
                profiler.span_with_args(
                    "snapshot.reconstruct.bundle.fanout_chunks",
                    bundle_fanout_trace_args(job, decoded_payload_bytes),
                )
            });
            let read_metrics = reader.stream_bundle_chunks_with_profiler(
                profiler,
                &required,
                |index, bytes| {
                    control.checkpoint()?;
                    let (entry, chunk) = required_chunks
                        .get(index)
                        .and_then(Option::as_ref)
                        .ok_or_else(|| {
                            Error::reconstruct(format!(
                                "bundle streamed unexpected unselected chunk {}",
                                index
                            ))
                        })?;
                    if chunk.raw_size != entry.raw_size
                        || chunk.compressed_size != entry.compressed_size
                    {
                        return Err(Error::reconstruct(format!(
                            "bundle chunk '{}' size mismatch in '{}': expected raw {}, compressed {}; table has raw {}, compressed {}",
                            entry.chunk_hash,
                            job.artifact.id(),
                            chunk.raw_size,
                            chunk.compressed_size,
                            entry.raw_size,
                            entry.compressed_size
                        )));
                    }
                    let hash_started_at = Instant::now();
                    let actual_hash =
                        crate::hash::ChunkHash::from_blake3(crate::hash::hash_chunk(bytes));
                    timings.chunk_verify += hash_started_at.elapsed();
                    timings.chunk_verify_bytes = timings
                        .chunk_verify_bytes
                        .saturating_add(bytes.len() as u64);
                    if actual_hash.to_string() != chunk.chunk_hash {
                        return Err(Error::HashMismatch {
                            expected: chunk.chunk_hash.clone(),
                            actual: actual_hash.to_string(),
                        });
                    }
                    write_reconstruct_targets(
                        chunk,
                        bytes,
                        output_handles,
                        &mut target_handles,
                        control,
                        &mut timings,
                    )?;
                    Ok(())
                },
            )?;
            let artifact_read_elapsed = read_metrics.read + read_metrics.decompress;
            timings.artifact_read = artifact_read_elapsed;
            timings.artifact_read_compressed_bytes = read_metrics.compressed_bytes;
            timings.artifact_read_decompressed_bytes = read_metrics.raw_bytes;
            timings.bundle_read = artifact_read_elapsed;
            drop(_fanout_span);
            timings.artifact_read_compressed_bytes = compressed_bytes;
            timings.bundle_read_compressed_bytes = compressed_bytes;
            timings.bundle_read_decompressed_bytes = timings.artifact_read_decompressed_bytes;
        }
        crate::ArtifactRef::LooseChunk { hash } => {
            if job.chunks.len() != 1 || !job.chunks.contains_key(hash) {
                return Err(Error::reconstruct(format!(
                    "loose artifact '{}' does not contain exactly its requested chunk",
                    hash
                )));
            }
            let chunk = job.chunks.get(hash).expect("validated loose chunk entry");
            if compressed_bytes != chunk.compressed_size {
                return Err(Error::reconstruct(format!(
                    "loose artifact '{}' has compressed size {}, expected {}",
                    hash, compressed_bytes, chunk.compressed_size
                )));
            }
            let (bytes, read_metrics) = crate::artifact::store::read_loose_chunk_with_metrics(
                &job.artifact_path,
                hash,
                chunk.raw_size,
                compression_profile,
                profiler,
            )?;
            timings.artifact_read = read_metrics.read
                + read_metrics.decompress
                + read_metrics.size_verify
                + read_metrics.hash_verify;
            timings.artifact_read_compressed_bytes = read_metrics.compressed_bytes;
            timings.artifact_read_decompressed_bytes = read_metrics.raw_bytes;
            timings.chunk_verify = read_metrics.hash_verify;
            timings.chunk_verify_bytes = read_metrics.raw_bytes;
            write_reconstruct_targets(
                chunk,
                &bytes,
                output_handles,
                &mut target_handles,
                control,
                &mut timings,
            )?;
        }
    }
    Ok(timings)
}

fn reconstruct_source_chunk_job(
    job: &ReconstructSourceChunkJob,
    file_plans: &[ReconstructPipelineFile],
    output_handles: &OutputHandleCache,
    control: &GutchOperationControl,
) -> Result<ReconstructTimings> {
    control.checkpoint()?;
    let _source_span = control.profile_span_with_args(
        "snapshot.reconstruct.job.source_chunk",
        source_chunk_trace_args(job),
    );
    let mut input = {
        let _open_span = control.profile_span("snapshot.reconstruct.source_chunk.open");
        File::open(&job.source_path)?
    };
    let mut bytes = vec![
        0u8;
        usize::try_from(job.raw_size).map_err(|_| {
            Error::reconstruct(format!(
                "source chunk '{}' is too large for this platform",
                job.chunk_hash
            ))
        })?
    ];
    let read_started_at = Instant::now();
    {
        let _read_span = control.profile_span_with_args(
            "snapshot.reconstruct.source_chunk.read",
            source_chunk_trace_args(job),
        );
        input.seek(SeekFrom::Start(job.source_offset))?;
        input.read_exact(&mut bytes)?;
    }
    let read_elapsed = read_started_at.elapsed();
    let hash_started_at = Instant::now();
    let actual_hash = {
        let _hash_span = control.profile_span_with_args(
            "snapshot.reconstruct.source_chunk.hash_verify",
            source_chunk_trace_args(job),
        );
        crate::hash::hash_hex(&crate::hash::hash_chunk(&bytes))
    };
    let hash_elapsed = hash_started_at.elapsed();
    if actual_hash != job.chunk_hash {
        return Err(Error::HashMismatch {
            expected: job.chunk_hash.clone(),
            actual: actual_hash,
        });
    }

    let mut timings = ReconstructTimings {
        artifact_read: read_elapsed,
        artifact_read_decompressed_bytes: bytes.len() as u64,
        chunk_verify: hash_elapsed,
        chunk_verify_bytes: bytes.len() as u64,
        ..ReconstructTimings::default()
    };
    let mut target_handles = HashMap::new();
    let rebuild_started_at = Instant::now();
    for target in &job.targets {
        control.checkpoint()?;
        let _target_span = control.profile_span_with_args(
            "snapshot.reconstruct.file.source_chunk",
            source_target_trace_args(job, target, &file_plans[target.file_index]),
        );
        let _write_span = control.profile_span_with_args(
            "snapshot.reconstruct.file.source_chunk.write",
            source_target_trace_args(job, target, &file_plans[target.file_index]),
        );
        let write_started_at = Instant::now();
        let output = cached_target_handle(output_handles, &mut target_handles, target.file_index)?;
        write_file_at(&output, target.file_offset, &bytes)?;
        timings.file_write += write_started_at.elapsed();
        timings.file_write_bytes = timings.file_write_bytes.saturating_add(bytes.len() as u64);
        timings.file_rebuild_bytes = timings
            .file_rebuild_bytes
            .saturating_add(bytes.len() as u64);
    }
    timings.file_rebuild = rebuild_started_at.elapsed();
    Ok(timings)
}

fn copy_source_file_job(
    file_index: usize,
    source_path: &Path,
    file_plans: &[ReconstructPipelineFile],
    output_paths: &[PathBuf],
    precomputed_file_hashes: &[Mutex<Option<String>>],
    control: &GutchOperationControl,
) -> Result<ReconstructTimings> {
    let file_plan = file_plans.get(file_index).ok_or_else(|| {
        Error::reconstruct(format!("source file job references file {file_index}"))
    })?;
    control.checkpoint()?;
    let _source_span = control.profile_span_with_args(
        "snapshot.reconstruct.job.source_file",
        source_file_trace_args(file_plan, source_path),
    );
    let mut input = {
        let _open_span = control.profile_span_with_args(
            "snapshot.reconstruct.source_file.open",
            source_file_trace_args(file_plan, source_path),
        );
        File::open(source_path)?
    };
    let mut output = {
        let _open_span = control.profile_span_with_args(
            "snapshot.reconstruct.source_file.output_open",
            crate::profile::file_trace_args(file_plan.path.clone(), file_plan.size),
        );
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&output_paths[file_index])?
    };
    let mut timings = ReconstructTimings::default();
    let mut buffer = vec![0u8; 1024 * 1024];
    let mut full_file_hasher = blake3::Hasher::new();
    let mut copied = 0u64;
    loop {
        control.checkpoint()?;
        let read = {
            let _read_span = control.profile_span_with_args(
                "snapshot.reconstruct.source_file.read",
                source_file_trace_args(file_plan, source_path),
            );
            input.read(&mut buffer)?
        };
        if read == 0 {
            break;
        }
        full_file_hasher.update(&buffer[..read]);
        let _write_span = control.profile_span_with_args(
            "snapshot.reconstruct.file.source_file.write",
            file_write_trace_args(file_plan, read as u64),
        );
        let write_started_at = Instant::now();
        output.write_all(&buffer[..read])?;
        timings.file_write += write_started_at.elapsed();
        timings.file_write_bytes = timings.file_write_bytes.saturating_add(read as u64);
        timings.file_rebuild_bytes = timings.file_rebuild_bytes.saturating_add(read as u64);
        copied = copied.saturating_add(read as u64);
    }
    {
        let _flush_span = control.profile_span_with_args(
            "snapshot.reconstruct.file.source_file.flush",
            crate::profile::file_trace_args(file_plan.path.clone(), file_plan.size),
        );
        let flush_started_at = Instant::now();
        output.flush()?;
        timings.file_write += flush_started_at.elapsed();
    }
    timings.file_rebuild = timings.file_write;
    if copied != file_plan.size {
        return Err(Error::reconstruct(format!(
            "copied source file '{}' has size {}, expected {}",
            file_plan.path, copied, file_plan.size
        )));
    }
    let actual_hash = crate::hash::hash_hex(&full_file_hasher.finalize());
    if actual_hash != file_plan.full_file_hash {
        return Err(Error::HashMismatch {
            expected: file_plan.full_file_hash.clone(),
            actual: actual_hash,
        });
    }
    let hash_slot = precomputed_file_hashes.get(file_index).ok_or_else(|| {
        Error::reconstruct(format!(
            "source file job references unknown file {file_index}"
        ))
    })?;
    *hash_slot
        .lock()
        .map_err(|_| Error::reconstruct("precomputed file hash is poisoned"))? = Some(actual_hash);
    Ok(timings)
}

fn write_reconstruct_targets(
    chunk: &ReconstructArtifactChunk,
    bytes: &[u8],
    output_handles: &OutputHandleCache,
    target_handles: &mut HashMap<usize, Arc<File>>,
    control: &GutchOperationControl,
    timings: &mut ReconstructTimings,
) -> Result<()> {
    if bytes.len() as u64 != chunk.raw_size {
        return Err(Error::reconstruct(format!(
            "artifact chunk '{}' has size {}, expected {}",
            chunk.chunk_hash,
            bytes.len(),
            chunk.raw_size
        )));
    }
    let rebuild_started_at = Instant::now();
    for target in &chunk.targets {
        control.checkpoint()?;
        let write_started_at = Instant::now();
        let output = cached_target_handle(output_handles, target_handles, target.file_index)?;
        write_file_at(&output, target.file_offset, bytes)?;
        timings.file_write += write_started_at.elapsed();
        timings.file_write_bytes = timings.file_write_bytes.saturating_add(bytes.len() as u64);
        timings.file_rebuild_bytes = timings
            .file_rebuild_bytes
            .saturating_add(bytes.len() as u64);
    }
    timings.file_rebuild += rebuild_started_at.elapsed();
    Ok(())
}

pub(super) struct OutputHandleCache {
    output_paths: Arc<Vec<PathBuf>>,
    output_sizes: Arc<Vec<u64>>,
    state: Mutex<OutputHandleCacheState>,
}

struct OutputHandleCacheState {
    handles: HashMap<usize, Arc<File>>,
    lru: VecDeque<usize>,
    max_open: usize,
}

const LOCAL_OUTPUT_HANDLE_CACHE_LIMIT: usize = 8;

impl OutputHandleCache {
    pub(super) fn new(output_paths: Arc<Vec<PathBuf>>, output_sizes: Arc<Vec<u64>>) -> Self {
        Self {
            output_paths,
            output_sizes,
            state: Mutex::new(OutputHandleCacheState {
                handles: HashMap::new(),
                lru: VecDeque::new(),
                max_open: 256,
            }),
        }
    }

    fn get_or_open(&self, file_index: usize) -> Result<Arc<File>> {
        if file_index >= self.output_paths.len() {
            return Err(Error::reconstruct(format!(
                "bundle target references unknown file index {}",
                file_index
            )));
        }
        {
            let mut state = self
                .state
                .lock()
                .map_err(|_| Error::reconstruct("output handle cache is poisoned"))?;
            if let Some(file) = state.handles.get(&file_index).cloned() {
                state.lru.retain(|index| *index != file_index);
                state.lru.push_back(file_index);
                return Ok(file);
            }
        }

        // Do filesystem work outside the cache mutex. A reconstruction can touch thousands of
        // files, and holding the mutex across open/set_len otherwise serializes every first write.
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .open(&self.output_paths[file_index])?;
        file.set_len(self.output_sizes[file_index])?;
        let file = Arc::new(file);

        let mut state = self
            .state
            .lock()
            .map_err(|_| Error::reconstruct("output handle cache is poisoned"))?;
        if let Some(existing) = state.handles.get(&file_index).cloned() {
            state.lru.retain(|index| *index != file_index);
            state.lru.push_back(file_index);
            return Ok(existing);
        }
        while state.handles.len() >= state.max_open {
            if let Some(oldest) = state.lru.pop_front() {
                state.handles.remove(&oldest);
            }
        }
        state.handles.insert(file_index, Arc::clone(&file));
        state.lru.push_back(file_index);
        Ok(file)
    }
}

fn cached_target_handle(
    output_handles: &OutputHandleCache,
    target_handles: &mut HashMap<usize, Arc<File>>,
    file_index: usize,
) -> Result<Arc<File>> {
    if let Some(file) = target_handles.get(&file_index) {
        return Ok(Arc::clone(file));
    }
    let file = output_handles.get_or_open(file_index)?;
    if target_handles.len() >= LOCAL_OUTPUT_HANDLE_CACHE_LIMIT
        && let Some(oldest_file_index) = target_handles.keys().next().copied()
    {
        target_handles.remove(&oldest_file_index);
    }
    target_handles.insert(file_index, Arc::clone(&file));
    Ok(file)
}

#[cfg(windows)]
fn write_file_at(file: &File, offset: u64, bytes: &[u8]) -> Result<()> {
    use std::os::windows::fs::FileExt;

    let mut written = 0usize;
    while written < bytes.len() {
        let count = file.seek_write(&bytes[written..], offset + written as u64)?;
        if count == 0 {
            return Err(Error::reconstruct("output file write returned zero bytes"));
        }
        written += count;
    }
    Ok(())
}

#[cfg(unix)]
fn write_file_at(file: &File, offset: u64, bytes: &[u8]) -> Result<()> {
    use std::os::unix::fs::FileExt;

    file.write_all_at(bytes, offset)?;
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn write_file_at(file: &File, offset: u64, bytes: &[u8]) -> Result<()> {
    let mut file = file.try_clone()?;
    file.seek(SeekFrom::Start(offset))?;
    file.write_all(bytes)?;
    Ok(())
}

fn reconstruct_artifact_trace_args(
    job: &ReconstructArtifactJob,
) -> std::collections::BTreeMap<String, serde_json::Value> {
    let logical_bytes = job
        .chunks
        .values()
        .map(|chunk| chunk.raw_size.saturating_mul(chunk.targets.len() as u64))
        .sum::<u64>();
    std::collections::BTreeMap::from([
        (
            "artifact_kind".to_string(),
            serde_json::json!(match &job.artifact {
                crate::ArtifactRef::Bundle { .. } => "bundle",
                crate::ArtifactRef::LooseChunk { .. } => "loose_chunk",
            }),
        ),
        (
            "artifact_id".to_string(),
            serde_json::json!(job.artifact.id()),
        ),
        (
            "artifact_path".to_string(),
            serde_json::json!(job.artifact_path.display().to_string()),
        ),
        (
            "required_chunk_count".to_string(),
            serde_json::json!(job.chunks.len()),
        ),
        (
            "logical_write_mib".to_string(),
            serde_json::json!(logical_bytes as f64 / (1024.0 * 1024.0)),
        ),
    ])
}

fn bundle_fanout_trace_args(
    job: &ReconstructArtifactJob,
    decoded_payload_bytes: u64,
) -> std::collections::BTreeMap<String, serde_json::Value> {
    let mut args = reconstruct_artifact_trace_args(job);
    args.insert(
        "decoded_payload_mib".to_string(),
        serde_json::json!(decoded_payload_bytes as f64 / (1024.0 * 1024.0)),
    );
    args.insert(
        "fanout_chunk_count".to_string(),
        serde_json::json!(job.chunks.len()),
    );
    args
}

fn source_chunk_trace_args(
    job: &ReconstructSourceChunkJob,
) -> std::collections::BTreeMap<String, serde_json::Value> {
    std::collections::BTreeMap::from([
        ("chunk_hash".to_string(), serde_json::json!(job.chunk_hash)),
        (
            "source_path".to_string(),
            serde_json::json!(job.source_path.display().to_string()),
        ),
        (
            "source_offset".to_string(),
            serde_json::json!(job.source_offset),
        ),
        (
            "raw_size_mib".to_string(),
            serde_json::json!(job.raw_size as f64 / (1024.0 * 1024.0)),
        ),
        (
            "target_count".to_string(),
            serde_json::json!(job.targets.len()),
        ),
    ])
}

fn source_target_trace_args(
    job: &ReconstructSourceChunkJob,
    target: &ReconstructTarget,
    file_plan: &ReconstructPipelineFile,
) -> std::collections::BTreeMap<String, serde_json::Value> {
    let mut args = crate::profile::file_trace_args(file_plan.path.clone(), file_plan.size);
    args.insert("chunk_hash".to_string(), serde_json::json!(job.chunk_hash));
    args.insert(
        "source_path".to_string(),
        serde_json::json!(job.source_path.display().to_string()),
    );
    args.insert(
        "source_offset".to_string(),
        serde_json::json!(job.source_offset),
    );
    args.insert(
        "raw_size_mib".to_string(),
        serde_json::json!(job.raw_size as f64 / (1024.0 * 1024.0)),
    );
    args.insert(
        "file_offset".to_string(),
        serde_json::json!(target.file_offset),
    );
    args
}

fn source_file_trace_args(
    file_plan: &ReconstructPipelineFile,
    source_path: &Path,
) -> std::collections::BTreeMap<String, serde_json::Value> {
    let mut args = crate::profile::file_trace_args(file_plan.path.clone(), file_plan.size);
    args.insert(
        "source_path".to_string(),
        serde_json::json!(source_path.display().to_string()),
    );
    args
}

fn file_write_trace_args(
    file_plan: &ReconstructPipelineFile,
    write_size: u64,
) -> std::collections::BTreeMap<String, serde_json::Value> {
    let mut args = crate::profile::file_trace_args(&file_plan.path, file_plan.size);
    args.insert(
        "write_size_mib".to_string(),
        serde_json::json!(write_size as f64 / (1024.0 * 1024.0)),
    );
    args
}

pub(super) struct ReconstructedFileVerification {
    pub(super) elapsed: Duration,
    pub(super) bytes: u64,
}

const PARALLEL_FULL_HASH_MIN_BYTES: u64 = 16 * 1024 * 1024;

pub(super) fn verify_reconstructed_output_file(
    file_index: usize,
    file_plan: &ReconstructPipelineFile,
    output_paths: &[PathBuf],
    precomputed_file_hashes: &[Mutex<Option<String>>],
    control: &GutchOperationControl,
    reconstruct_worker_count: usize,
) -> Result<ReconstructedFileVerification> {
    control.checkpoint()?;
    let _file_span = control.profile_span_with_args(
        "snapshot.reconstruct.file",
        reconstruct_file_trace_args(file_plan),
    );
    let precomputed_hash = precomputed_file_hashes
        .get(file_index)
        .ok_or_else(|| {
            Error::reconstruct(format!("verification references unknown file {file_index}"))
        })?
        .lock()
        .map_err(|_| Error::reconstruct("precomputed file hash is poisoned"))?
        .clone();
    let hash_forwarded = precomputed_hash.is_some();
    let hash_mode = if precomputed_hash.is_some() {
        "copy_forward"
    } else if file_plan.size < PARALLEL_FULL_HASH_MIN_BYTES {
        "stream"
    } else if reconstruct_worker_count <= 1 {
        "mmap_rayon"
    } else {
        "mmap"
    };
    let started_at = Instant::now();
    let output_path = &output_paths[file_index];
    let actual_size = std::fs::metadata(output_path)?.len();
    if actual_size != file_plan.size {
        return Err(Error::reconstruct(format!(
            "reconstructed size {} does not match expected {} for {}",
            actual_size, file_plan.size, file_plan.path
        )));
    }
    let hash_started_at = Instant::now();
    let actual_hash = if let Some(precomputed_hash) = precomputed_hash {
        control.profile_event_with_args(
            "snapshot.reconstruct.file.full_hash.forwarded",
            full_hash_trace_args(file_plan, hash_mode, reconstruct_worker_count),
        );
        precomputed_hash
    } else if hash_mode == "mmap_rayon" {
        let _hash_span = control.profile_span_with_args(
            "snapshot.reconstruct.file.full_hash",
            full_hash_trace_args(file_plan, hash_mode, reconstruct_worker_count),
        );
        let _parallel_hash_span = control.profile_span_with_args(
            "snapshot.reconstruct.file.full_hash.parallel",
            full_hash_trace_args(file_plan, hash_mode, reconstruct_worker_count),
        );
        let mut hasher = blake3::Hasher::new();
        hasher.update_mmap_rayon(output_path)?;
        crate::hash::hash_hex(&hasher.finalize())
    } else if hash_mode == "mmap" {
        let _hash_span = control.profile_span_with_args(
            "snapshot.reconstruct.file.full_hash",
            full_hash_trace_args(file_plan, hash_mode, reconstruct_worker_count),
        );
        let _mmap_hash_span = control.profile_span_with_args(
            "snapshot.reconstruct.file.full_hash.mmap",
            full_hash_trace_args(file_plan, hash_mode, reconstruct_worker_count),
        );
        let mut hasher = blake3::Hasher::new();
        hasher.update_mmap(output_path)?;
        crate::hash::hash_hex(&hasher.finalize())
    } else {
        let _hash_span = control.profile_span_with_args(
            "snapshot.reconstruct.file.full_hash",
            full_hash_trace_args(file_plan, hash_mode, reconstruct_worker_count),
        );
        let mut input = File::open(output_path)?;
        let mut hasher = blake3::Hasher::new();
        let mut buffer = vec![0u8; 1024 * 1024];
        loop {
            control.checkpoint()?;
            let read = input.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        crate::hash::hash_hex(&hasher.finalize())
    };
    let hash_elapsed = if hash_forwarded {
        Duration::ZERO
    } else {
        hash_started_at.elapsed()
    };
    let hash_elapsed_seconds = hash_elapsed.as_secs_f64();
    let mut hash_complete_args =
        full_hash_trace_args(file_plan, hash_mode, reconstruct_worker_count);
    hash_complete_args.insert(
        "hash_elapsed_ms".to_string(),
        serde_json::json!(hash_elapsed_seconds * 1000.0),
    );
    hash_complete_args.insert(
        "hash_speed_mib_s".to_string(),
        serde_json::json!(if hash_elapsed_seconds > 0.0 {
            (file_plan.size as f64 / (1024.0 * 1024.0)) / hash_elapsed_seconds
        } else {
            0.0
        }),
    );
    control.profile_event_with_args(
        "snapshot.reconstruct.file.full_hash.complete",
        hash_complete_args,
    );
    if actual_hash != file_plan.full_file_hash {
        return Err(Error::HashMismatch {
            expected: file_plan.full_file_hash.clone(),
            actual: actual_hash,
        });
    }
    Ok(ReconstructedFileVerification {
        elapsed: started_at.elapsed(),
        bytes: actual_size,
    })
}

fn full_hash_trace_args(
    file_plan: &ReconstructPipelineFile,
    hash_mode: &str,
    reconstruct_worker_count: usize,
) -> std::collections::BTreeMap<String, serde_json::Value> {
    std::collections::BTreeMap::from([
        (
            "size_mib".to_string(),
            serde_json::json!(file_plan.size as f64 / (1024.0 * 1024.0)),
        ),
        ("hash_mode".to_string(), serde_json::json!(hash_mode)),
        (
            "hash_threshold_mib".to_string(),
            serde_json::json!(PARALLEL_FULL_HASH_MIN_BYTES as f64 / (1024.0 * 1024.0)),
        ),
        (
            "configured_workers".to_string(),
            serde_json::json!(reconstruct_worker_count),
        ),
    ])
}

fn reconstruct_file_trace_args(
    file_plan: &ReconstructPipelineFile,
) -> std::collections::BTreeMap<String, serde_json::Value> {
    let mut args = crate::profile::file_trace_args(&file_plan.path, file_plan.size);
    args.insert(
        "required_distinct_chunks".to_string(),
        serde_json::json!(file_plan.trace_metadata.required_distinct_chunks),
    );
    args.insert(
        "required_distinct_artifacts".to_string(),
        serde_json::json!(file_plan.trace_metadata.required_distinct_artifacts),
    );
    args.insert(
        "required_distinct_bundles".to_string(),
        serde_json::json!(file_plan.trace_metadata.required_distinct_bundles),
    );
    args.insert(
        "required_distinct_loose_chunks".to_string(),
        serde_json::json!(file_plan.trace_metadata.required_distinct_loose_chunks),
    );
    args
}

pub(super) fn reconstruct_file_trace_metadata(
    reconstruction: &ReconstructPipelineReconstruction,
) -> ReconstructFileTraceMetadata {
    let mut chunk_hashes = HashSet::new();
    let mut artifacts = HashSet::new();
    let mut bundle_artifacts = HashSet::new();
    let mut loose_chunk_artifacts = HashSet::new();
    let ReconstructPipelineReconstruction::ReconstructFromParts { chunks } = reconstruction else {
        return ReconstructFileTraceMetadata::default();
    };

    for chunk in chunks {
        match chunk {
            ReconstructPipelineChunkInput::Source { chunk_hash } => {
                chunk_hashes.insert(chunk_hash.as_str());
            }
            ReconstructPipelineChunkInput::Artifact(chunk) => {
                chunk_hashes.insert(chunk.chunk_hash.as_str());
                artifacts.insert(&chunk.artifact);
                match &chunk.artifact {
                    crate::ArtifactRef::Bundle { .. } => {
                        bundle_artifacts.insert(&chunk.artifact);
                    }
                    crate::ArtifactRef::LooseChunk { .. } => {
                        loose_chunk_artifacts.insert(&chunk.artifact);
                    }
                }
            }
        }
    }

    ReconstructFileTraceMetadata {
        required_distinct_chunks: chunk_hashes.len(),
        required_distinct_artifacts: artifacts.len(),
        required_distinct_bundles: bundle_artifacts.len(),
        required_distinct_loose_chunks: loose_chunk_artifacts.len(),
    }
}

pub(super) fn build_source_chunk_locations(
    source: SnapshotReconstructSource<'_>,
) -> Result<HashMap<String, SourceChunkLocation>> {
    let mut chunk_locations = HashMap::new();
    for file_manifest in source.snapshot_manifest.files() {
        let typed_file = crate::manifest::runtime::typed_file_manifest(file_manifest)?;
        let source_path = crate::path_guard::resolve_manifest_relative_path(
            source.destination_dir,
            &file_manifest.path,
        )?;
        let mut offset = 0u64;
        for chunk_ref in typed_file.chunks {
            chunk_locations
                .entry(chunk_ref.hash_hex.to_string())
                .or_insert_with(|| SourceChunkLocation {
                    path: source_path.clone(),
                    offset,
                    raw_size: chunk_ref.raw_size,
                });
            offset = offset.checked_add(chunk_ref.raw_size).ok_or_else(|| {
                Error::manifest(format!(
                    "source file '{}' chunk sizes overflow u64",
                    file_manifest.path
                ))
            })?;
        }
    }
    Ok(chunk_locations)
}

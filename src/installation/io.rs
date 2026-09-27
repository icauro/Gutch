use super::types::SnapshotReconstructSource;
use crate::profile::ProfileInstant;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::Read;
use std::io::Seek;
use std::io::SeekFrom;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use super::jobs::ReconstructArtifactChunk;
use super::jobs::ReconstructArtifactJob;
use super::jobs::ReconstructJob;
use super::jobs::ReconstructPipelineFile;
use super::jobs::ReconstructSourceChunkJob;
use super::jobs::ReconstructTarget;
use super::jobs::SourceChunkLocation;
use crate::Error;
use crate::Result;
use crate::installation::types::ReconstructTimings;
use crate::progress::GutchOperationControl;

pub(super) fn reconstruct_job(
    job: &ReconstructJob,
    file_plans: &[ReconstructPipelineFile],
    output_paths: &[PathBuf],
    precomputed_file_hashes: &[Mutex<Option<crate::artifact::hash::ChunkHash>>],
    output_handles: &OutputHandleCache,
    source_handles: &SourceHandleCache,
    compression_profile: &crate::manifest::CompressionProfile,
    control: &GutchOperationControl,
) -> Result<ReconstructTimings> {
    match job {
        ReconstructJob::Artifact(job) => {
            reconstruct_artifact_job(job, output_handles, compression_profile, control)
        }
        ReconstructJob::SourceChunk(job) => {
            reconstruct_source_chunk_job(job, file_plans, output_handles, source_handles, control)
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
        crate::repo::artifacts::ArtifactRef::Bundle { .. } => {
            "installation.bundle.decode_verify_and_write"
        }
        crate::repo::artifacts::ArtifactRef::LooseChunk { .. } => {
            "installation.reconstruct.job.loose_chunk"
        }
    };
    let _artifact_span =
        control.profile_span_with_args(artifact_span_name, || reconstruct_artifact_trace_args(job));
    let profiler = control.profiler();
    let compressed_bytes = std::fs::metadata(&job.artifact_path)?.len();
    let mut timings = ReconstructTimings::default();
    let mut target_handles = HashMap::new();

    match &job.artifact {
        crate::repo::artifacts::ArtifactRef::Bundle { .. } => {
            let mut reader = crate::artifact::bundle::BundleReader::open(
                File::open(&job.artifact_path)?,
                compression_profile,
            )?;
            let mut required = vec![false; reader.entries().len()];
            let mut required_chunks = vec![None; reader.entries().len()];
            for (index, entry) in reader.entries().iter().enumerate() {
                if let Some(chunk) = job
                    .chunks
                    .binary_search_by_key(&entry.chunk_hash, |chunk| chunk.chunk_hash)
                    .ok()
                    .map(|index| &job.chunks[index])
                {
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
                    let hash_started_at = ProfileInstant::detailed(control.profiler());
                    let actual_hash =
                        crate::artifact::hash::ChunkHash::from_blake3(crate::artifact::hash::hash_chunk(bytes));
                    timings.chunk_verify += hash_started_at.elapsed();
                    timings.chunk_verify_bytes = timings
                        .chunk_verify_bytes
                        .saturating_add(bytes.len() as u64);
                    if actual_hash != chunk.chunk_hash {
                        return Err(Error::HashMismatch {
                            expected: chunk.chunk_hash.to_hex(),
                            actual: actual_hash.to_string(),
                        });
                    }
                    write_reconstruct_targets(
                        chunk,
                        &job.targets[chunk.target_range.clone()],
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

            timings.artifact_read_compressed_bytes = compressed_bytes;
            timings.bundle_read_compressed_bytes = compressed_bytes;
            timings.bundle_read_decompressed_bytes = timings.artifact_read_decompressed_bytes;
        }
        crate::repo::artifacts::ArtifactRef::LooseChunk { hash } => {
            if job.chunks.len() != 1
                || job.chunks[0].chunk_hash != crate::artifact::hash::ChunkHash::parse_hex(hash)?
            {
                return Err(Error::reconstruct(format!(
                    "loose artifact '{}' does not contain exactly its requested chunk",
                    hash
                )));
            }
            let chunk = &job.chunks[0];
            if compressed_bytes != u64::from(chunk.compressed_size) {
                return Err(Error::reconstruct(format!(
                    "loose artifact '{}' has compressed size {}, expected {}",
                    hash, compressed_bytes, chunk.compressed_size
                )));
            }
            let (bytes, read_metrics) = crate::artifact::loose::read_loose_chunk_with_metrics(
                &job.artifact_path,
                hash,
                u64::from(chunk.raw_size),
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
                &job.targets[chunk.target_range.clone()],
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
    _file_plans: &[ReconstructPipelineFile],
    output_handles: &OutputHandleCache,
    source_handles: &SourceHandleCache,
    control: &GutchOperationControl,
) -> Result<ReconstructTimings> {
    control.checkpoint()?;
    let _source_job_span =
        control.profile_span_with_args("installation.source_chunk.read_verify_and_write", || {
            std::collections::BTreeMap::from([
                ("size_bytes".into(), serde_json::json!(job.raw_size)),
                ("target_count".into(), serde_json::json!(job.targets.len())),
            ])
        });

    let mut bytes = vec![
        0u8;
        usize::try_from(job.raw_size).map_err(|_| {
            Error::reconstruct(format!(
                "source chunk '{}' is too large for this platform",
                job.chunk_hash
            ))
        })?
    ];
    let read_started_at = ProfileInstant::detailed(control.profiler());
    {
        source_handles.read_range(&job.source_path, job.source_offset, &mut bytes)?;
    }
    let read_elapsed = read_started_at.elapsed();
    let hash_started_at = ProfileInstant::detailed(control.profiler());
    let actual_hash =
        { crate::artifact::hash::hash_hex(&crate::artifact::hash::hash_chunk(&bytes)) };
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
    let rebuild_started_at = ProfileInstant::detailed(control.profiler());
    for target in &job.targets {
        control.checkpoint()?;

        let write_started_at = ProfileInstant::detailed(control.profiler());
        let output = cached_target_handle(output_handles, &mut target_handles, target.file_index)?;
        write_file_at(&output, target.file_offset, &bytes, control)?;
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
    precomputed_file_hashes: &[Mutex<Option<crate::artifact::hash::ChunkHash>>],
    control: &GutchOperationControl,
) -> Result<ReconstructTimings> {
    let file_plan = file_plans.get(file_index).ok_or_else(|| {
        Error::reconstruct(format!("source file job references file {file_index}"))
    })?;
    control.checkpoint()?;
    let _source_span = control.profile_span_with_args("installation.file.copy_and_hash", || {
        crate::profile::file_trace_args(&file_plan.path, file_plan.size)
    });
    let mut input = { File::open(source_path)? };
    let mut output = {
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
        let read = { input.read(&mut buffer)? };
        if read == 0 {
            break;
        }
        full_file_hasher.update(&buffer[..read]);

        let write_started_at = ProfileInstant::detailed(control.profiler());
        let mut offset = 0;
        while offset < read {
            let count = output.write(&buffer[offset..read])?;
            if count == 0 {
                return Err(Error::reconstruct("output write returned zero bytes"));
            }
            control.written(count as u64);
            offset += count;
        }
        timings.file_write += write_started_at.elapsed();
        timings.file_write_bytes = timings.file_write_bytes.saturating_add(read as u64);
        timings.file_rebuild_bytes = timings.file_rebuild_bytes.saturating_add(read as u64);
        copied = copied.saturating_add(read as u64);
    }
    {
        let flush_started_at = ProfileInstant::detailed(control.profiler());
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
    let actual_hash = crate::artifact::hash::ChunkHash::from_blake3(full_file_hasher.finalize());
    if actual_hash != file_plan.full_file_hash {
        return Err(Error::HashMismatch {
            expected: file_plan.full_file_hash.to_hex(),
            actual: actual_hash.to_hex(),
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
    targets: &[ReconstructTarget],
    bytes: &[u8],
    output_handles: &OutputHandleCache,
    target_handles: &mut HashMap<usize, Arc<File>>,
    control: &GutchOperationControl,
    timings: &mut ReconstructTimings,
) -> Result<()> {
    if bytes.len() as u64 != u64::from(chunk.raw_size) {
        return Err(Error::reconstruct(format!(
            "artifact chunk '{}' has size {}, expected {}",
            chunk.chunk_hash,
            bytes.len(),
            chunk.raw_size
        )));
    }
    let rebuild_started_at = ProfileInstant::detailed(control.profiler());
    for target in targets {
        control.checkpoint()?;
        let write_started_at = ProfileInstant::detailed(control.profiler());
        let output = cached_target_handle(output_handles, target_handles, target.file_index)?;
        write_file_at(&output, target.file_offset, bytes, control)?;
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
    independent_handles: Vec<bool>,
    state: Mutex<OutputHandleCacheState>,
}

#[derive(Default)]
pub(super) struct SourceHandleCache {
    handles: Mutex<HashMap<PathBuf, Arc<Mutex<File>>>>,
}
impl SourceHandleCache {
    fn read_range(&self, path: &std::path::Path, offset: u64, bytes: &mut [u8]) -> Result<()> {
        let input = {
            let mut handles = self
                .handles
                .lock()
                .map_err(|_| Error::reconstruct("source cache poisoned"))?;
            if let Some(file) = handles.get(path) {
                Arc::clone(file)
            } else {
                if handles.len() >= 64
                    && let Some(key) = handles.keys().next().cloned()
                {
                    handles.remove(&key);
                }
                let file = Arc::new(Mutex::new(File::open(path)?));
                handles.insert(path.to_path_buf(), Arc::clone(&file));
                file
            }
        };
        let mut input = input
            .lock()
            .map_err(|_| Error::reconstruct("source handle poisoned"))?;
        if input.stream_position()? != offset {
            input.seek(SeekFrom::Start(offset))?;
        }
        input.read_exact(bytes)?;
        Ok(())
    }
}

struct OutputHandleCacheState {
    handles: HashMap<usize, Arc<File>>,
    lru: VecDeque<usize>,
    max_open: usize,
}

const LOCAL_OUTPUT_HANDLE_CACHE_LIMIT: usize = 8;
const INDEPENDENT_OUTPUT_MIN_BYTES: u64 = 64 * 1024 * 1024;

impl OutputHandleCache {
    pub(super) fn new(
        output_paths: Arc<Vec<PathBuf>>,
        output_sizes: Arc<Vec<u64>>,
        producer_counts: &[usize],
        worker_count: usize,
    ) -> Self {
        // These sizes and producer counts already exist for verification scheduling.
        // Independent Windows file objects help concurrent large-file writes, but
        // reopening tiny files costs more than it saves. No I/O probe is needed.
        let independent_handles = output_sizes
            .iter()
            .zip(producer_counts)
            .map(|(&size, &producers)| {
                cfg!(windows)
                    && worker_count > 1
                    && producers > 1
                    && size >= INDEPENDENT_OUTPUT_MIN_BYTES
            })
            .collect();
        Self {
            output_paths,
            output_sizes,
            independent_handles,
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
            .truncate(false)
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
    let file = if output_handles.independent_handles[file_index] {
        // Creation and sizing stay centralized. This handle only opens an already
        // prepared file; it must never truncate data written by another job.
        Arc::new(
            OpenOptions::new()
                .write(true)
                .open(&output_handles.output_paths[file_index])?,
        )
    } else {
        file
    };
    if target_handles.len() >= LOCAL_OUTPUT_HANDLE_CACHE_LIMIT
        && let Some(oldest_file_index) = target_handles.keys().next().copied()
    {
        target_handles.remove(&oldest_file_index);
    }
    target_handles.insert(file_index, Arc::clone(&file));
    Ok(file)
}

#[cfg(windows)]
fn write_file_at(
    file: &File,
    offset: u64,
    bytes: &[u8],
    control: &GutchOperationControl,
) -> Result<()> {
    use std::os::windows::fs::FileExt;

    let mut written = 0usize;
    while written < bytes.len() {
        let count = file.seek_write(&bytes[written..], offset + written as u64)?;
        if count == 0 {
            return Err(Error::reconstruct("output file write returned zero bytes"));
        }
        written += count;
        control.written(count as u64);
    }
    Ok(())
}

#[cfg(unix)]
fn write_file_at(
    file: &File,
    offset: u64,
    bytes: &[u8],
    control: &GutchOperationControl,
) -> Result<()> {
    use std::os::unix::fs::FileExt;

    let mut written = 0;
    while written < bytes.len() {
        let count = file.write_at(&bytes[written..], offset + written as u64)?;
        if count == 0 {
            return Err(Error::reconstruct("output write returned zero bytes"));
        }
        written += count;
        control.written(count as u64);
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn write_file_at(
    file: &File,
    offset: u64,
    bytes: &[u8],
    control: &GutchOperationControl,
) -> Result<()> {
    let mut file = file.try_clone()?;
    file.seek(SeekFrom::Start(offset))?;
    let mut written = 0;
    while written < bytes.len() {
        let count = file.write(&bytes[written..])?;
        if count == 0 {
            return Err(Error::reconstruct("output write returned zero bytes"));
        }
        written += count;
        control.written(count as u64);
    }
    Ok(())
}

fn reconstruct_artifact_trace_args(
    job: &ReconstructArtifactJob,
) -> std::collections::BTreeMap<String, serde_json::Value> {
    let logical_bytes = job
        .chunks
        .iter()
        .map(|chunk| u64::from(chunk.raw_size).saturating_mul(chunk.target_range.len() as u64))
        .sum::<u64>();
    std::collections::BTreeMap::from([
        (
            "artifact_kind".to_string(),
            serde_json::json!(match &job.artifact {
                crate::repo::artifacts::ArtifactRef::Bundle { .. } => "bundle",
                crate::repo::artifacts::ArtifactRef::LooseChunk { .. } => "loose_chunk",
            }),
        ),
        (
            "artifact_id".to_string(),
            serde_json::json!(job.artifact.id()),
        ),
        (
            "required_chunk_count".to_string(),
            serde_json::json!(job.chunks.len()),
        ),
        (
            "logical_write_bytes".to_string(),
            serde_json::json!(logical_bytes),
        ),
    ])
}

pub(super) struct ReconstructedFileVerification {
    pub(super) elapsed: Duration,
    pub(super) bytes: u64,
}

pub(super) fn verify_reconstructed_output_file(
    file_index: usize,
    file_plan: &ReconstructPipelineFile,
    output_paths: &[PathBuf],
    precomputed_file_hashes: &[Mutex<Option<crate::artifact::hash::ChunkHash>>],
    control: &GutchOperationControl,
    reconstruct_worker_count: usize,
) -> Result<ReconstructedFileVerification> {
    control.checkpoint()?;

    let precomputed_hash = *precomputed_file_hashes
        .get(file_index)
        .ok_or_else(|| {
            Error::reconstruct(format!("verification references unknown file {file_index}"))
        })?
        .lock()
        .map_err(|_| Error::reconstruct("precomputed file hash is poisoned"))?;
    let concurrent_file_count = reconstruct_worker_count.min(output_paths.len());
    let hash_mode = crate::file_integrity::select_full_file_hash_mode(
        file_plan.size,
        precomputed_hash.is_some(),
        concurrent_file_count,
    );
    let started_at = ProfileInstant::detailed(control.profiler());
    let output_path = &output_paths[file_index];
    let actual_size = std::fs::metadata(output_path)?.len();
    if actual_size != file_plan.size {
        return Err(Error::reconstruct(format!(
            "reconstructed size {} does not match expected {} for {}",
            actual_size, file_plan.size, file_plan.path
        )));
    }
    let _hash_span = if precomputed_hash.is_some() {
        control
            .profile_event_with_args("installation.reconstruct.file.full_hash.forwarded", || {
                full_hash_trace_args(file_plan, hash_mode.as_str(), reconstruct_worker_count)
            });
        None
    } else {
        control.profile_span_with_args("installation.file.verify", || {
            full_hash_trace_args(file_plan, hash_mode.as_str(), reconstruct_worker_count)
        })
    };
    crate::file_integrity::verify_full_file_integrity(
        output_path,
        file_plan.size,
        file_plan.full_file_hash,
        precomputed_hash,
        concurrent_file_count,
        control,
    )?;
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
        ("size_bytes".to_string(), serde_json::json!(file_plan.size)),
        ("hash_mode".to_string(), serde_json::json!(hash_mode)),
        (
            "hash_threshold_bytes".to_string(),
            serde_json::json!(crate::file_integrity::PARALLEL_FULL_HASH_MIN_BYTES),
        ),
        (
            "configured_workers".to_string(),
            serde_json::json!(reconstruct_worker_count),
        ),
    ])
}

pub(super) fn build_source_chunk_locations(
    source: SnapshotReconstructSource<'_>,
) -> Result<HashMap<String, SourceChunkLocation>> {
    let mut chunk_locations = HashMap::new();
    for file_manifest in source.snapshot_manifest.files() {
        let typed_file = crate::manifest::typed::typed_file_manifest(file_manifest)?;
        let source_path = crate::filesystem::paths::resolve_manifest_relative_path(
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

#[cfg(test)]
mod output_handle_tests {
    use super::*;

    #[test]
    fn output_handle_selection_preserves_ranges_and_job_local_reuse() {
        for (size, producers, workers, independent) in [
            (4096, 2, 4, false),
            (INDEPENDENT_OUTPUT_MIN_BYTES - 1, 2, 4, false),
            (INDEPENDENT_OUTPUT_MIN_BYTES, 1, 4, false),
            (INDEPENDENT_OUTPUT_MIN_BYTES, 2, 1, false),
            (INDEPENDENT_OUTPUT_MIN_BYTES, 2, 4, cfg!(windows)),
        ] {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("output");
            let cache = OutputHandleCache::new(
                Arc::new(vec![path.clone()]),
                Arc::new(vec![size]),
                &[producers],
                workers,
            );
            let mut first_job = HashMap::new();
            let first = cached_target_handle(&cache, &mut first_job, 0).unwrap();
            write_file_at(&first, size - 4, b"tail", &Default::default()).unwrap();
            let mut second_job = HashMap::new();
            let second = cached_target_handle(&cache, &mut second_job, 0).unwrap();
            assert_eq!(!Arc::ptr_eq(&first, &second), independent);
            assert!(Arc::ptr_eq(
                &first,
                &cached_target_handle(&cache, &mut first_job, 0).unwrap()
            ));
            std::thread::scope(|scope| {
                scope.spawn(|| write_file_at(&first, 0, b"head", &Default::default()).unwrap());
                scope.spawn(|| write_file_at(&second, 4, b"next", &Default::default()).unwrap());
            });
            let mut file = File::open(&path).unwrap();
            assert_eq!(file.metadata().unwrap().len(), size);
            let mut bytes = [0; 8];
            file.read_exact(&mut bytes).unwrap();
            assert_eq!(&bytes, b"headnext");
            file.seek(SeekFrom::Start(size - 4)).unwrap();
            file.read_exact(&mut bytes[..4]).unwrap();
            assert_eq!(&bytes[..4], b"tail");
        }
    }
}

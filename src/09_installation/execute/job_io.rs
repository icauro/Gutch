//! Reads, verifies and writes each reconstruction job's data.

use super::handles::ReconstructionOutputWriter;
use super::handles::{OutputHandleCache, SourceHandleCache, cached_target_handle, write_file_at};
use crate::installation::contracts::SnapshotReconstructSource;
use crate::profiling::ProfileInstant;
use std::collections::HashMap;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::Read;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
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
use crate::artifact::bundle::BundleReader;
use crate::artifact::hash::ChunkHash;
use crate::artifact::loose::read_loose_chunk_with_metrics;
use crate::artifact::profiles::CompressionProfile;
use crate::artifact::reference::ArtifactRef;
use crate::filesystem::paths::resolve_manifest_relative_path;
use crate::installation::contracts::ReconstructTimings;
use crate::manifest::typed::typed_file_manifest;
use crate::profiling::file_trace_args;
use crate::progress::OperationControl;
use std::collections::BTreeMap;
use std::collections::HashSet;
use std::fs;
use std::sync::Arc;

pub(crate) struct ReconstructJobRequest<'a> {
    pub(crate) job: &'a ReconstructJob,
    pub(crate) file_plans: &'a [ReconstructPipelineFile],
    pub(crate) output_paths: &'a [PathBuf],
    pub(crate) output_handles: &'a OutputHandleCache,
    pub(crate) source_handles: &'a SourceHandleCache,
    pub(crate) compression_profile: &'a CompressionProfile,
    pub(crate) control: &'a OperationControl,
}

pub(crate) fn reconstruct_job(request: ReconstructJobRequest<'_>) -> Result<ReconstructTimings> {
    let ReconstructJobRequest {
        job,
        file_plans,
        output_paths,
        output_handles,
        source_handles,
        compression_profile,
        control,
    } = request;
    match job {
        ReconstructJob::Artifact(job) => {
            reconstruct_artifact_job(job, output_handles, compression_profile, control).map_err(
                |e| e.with_io_context("reading reconstruction artifact", &job.artifact_path),
            )
        }
        ReconstructJob::SourceChunk(job) => {
            reconstruct_source_chunk_job(job, output_handles, source_handles, control)
        }
        ReconstructJob::CopySourceFile {
            file_index,
            source_path,
        } => copy_source_file_job(*file_index, source_path, file_plans, output_paths, control),
    }
}

fn reconstruct_artifact_job(
    job: &ReconstructArtifactJob,
    output_handles: &OutputHandleCache,
    compression_profile: &CompressionProfile,
    control: &OperationControl,
) -> Result<ReconstructTimings> {
    control.checkpoint()?;
    let artifact_span_name = match &job.artifact {
        ArtifactRef::Bundle { .. } => "installation.bundle.decode_verify_and_write",
        ArtifactRef::LooseChunk { .. } => "installation.reconstruct.job.loose_chunk",
    };
    let _artifact_span =
        control.profile_span_with_args(artifact_span_name, || reconstruct_artifact_trace_args(job));
    let profiler = control.profiler();
    let compressed_bytes = fs::metadata(&job.artifact_path)
        .map_err(|e| Error::io_context("reading artifact metadata", &job.artifact_path, e))?
        .len();
    let mut timings = ReconstructTimings::default();
    let mut output_writer = ReconstructionOutputWriter::new(output_handles, control);

    match &job.artifact {
        ArtifactRef::Bundle { .. } => {
            let mut reader = BundleReader::open(
                File::open(&job.artifact_path).map_err(|e| {
                    Error::io_context("opening reconstruction artifact", &job.artifact_path, e)
                })?,
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
            // Each worker reads a bundle once and verifies selected chunks before
            // positional fan-out. The decoder retains one chunk; the output writer
            // may retain one additional bounded range for coalescing.

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
                    if chunk.raw_size != entry.raw_size {
                        return Err(Error::reconstruct(format!(
                            "bundle chunk '{}' size mismatch in '{}': expected raw {}, table has raw {}",
                            entry.chunk_hash,
                            job.artifact.id(),
                            chunk.raw_size,
                            entry.raw_size,
                        )));
                    }
                    let hash_started_at = ProfileInstant::detailed(control.profiler());
                    let actual_hash =
                        ChunkHash::of(bytes);
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
                        &mut output_writer,
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
        ArtifactRef::LooseChunk { hash } => {
            if job.chunks.len() != 1 || job.chunks[0].chunk_hash != ChunkHash::parse_hex(hash)? {
                return Err(Error::reconstruct(format!(
                    "loose artifact '{}' does not contain exactly its requested chunk",
                    hash
                )));
            }
            let chunk = &job.chunks[0];
            if compressed_bytes != u64::from(job.encoded_size) {
                return Err(Error::reconstruct(format!(
                    "loose artifact '{}' has compressed size {}, expected {}",
                    hash, compressed_bytes, job.encoded_size
                )));
            }
            let (bytes, read_metrics) = read_loose_chunk_with_metrics(
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
                &mut output_writer,
                control,
                &mut timings,
            )?;
        }
    }
    // A terminal success must never expose output ranges still held in memory.
    let flush_started_at = ProfileInstant::detailed(control.profiler());
    output_writer.flush()?;
    let flush_elapsed = flush_started_at.elapsed();
    timings.file_write += flush_elapsed;
    timings.file_rebuild += flush_elapsed;
    Ok(timings)
}

fn reconstruct_source_chunk_job(
    job: &ReconstructSourceChunkJob,
    output_handles: &OutputHandleCache,
    source_handles: &SourceHandleCache,
    control: &OperationControl,
) -> Result<ReconstructTimings> {
    control.checkpoint()?;
    let _source_job_span =
        control.profile_span_with_args("installation.source_chunk.read_verify_and_write", || {
            BTreeMap::from([
                ("size_bytes".into(), serde_json::json!(job.raw_size)),
                (
                    "target_count".into(),
                    serde_json::json!(job.targets().count()),
                ),
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
    verify_and_write_source_chunk(job, &bytes, read_elapsed, output_handles, control, None)
}

// Workers admit a nonempty source-only group of at most 64 KiB. That buffer
// and the bounded output buffer fit within the existing 1 MiB source-I/O
// reservation. Never read across excluded source ranges.
pub(crate) fn reconstruct_source_chunk_batch(
    jobs: &[ReconstructJob],
    output_handles: &OutputHandleCache,
    source_handles: &SourceHandleCache,
    control: &OperationControl,
) -> Result<Vec<ReconstructTimings>> {
    let chunks: Vec<&ReconstructSourceChunkJob> = jobs
        .iter()
        .map(|job| match job {
            ReconstructJob::SourceChunk(chunk) => chunk,
            _ => unreachable!("only source chunks are batched"),
        })
        .collect();
    let first = chunks[0];
    // Admission batches only one contiguous range of one source file, so a
    // single read covers every chunk; each chunk is still hash-verified below.
    let mut end = first.source_offset;
    for chunk in &chunks {
        debug_assert!(chunk.source_path == first.source_path && chunk.source_offset == end);
        end = end
            .checked_add(u64::from(chunk.raw_size))
            .ok_or_else(|| Error::reconstruct("source batch offset overflows u64"))?;
    }
    control.checkpoint()?;
    let size = usize::try_from(end - first.source_offset)
        .map_err(|_| Error::reconstruct("source batch is too large for this platform"))?;
    let mut bytes = vec![0; size];
    let read_started_at = ProfileInstant::detailed(control.profiler());
    {
        let _read_span = control.profile_span("installation.source_batch.read");
        source_handles.read_range(&first.source_path, first.source_offset, &mut bytes)?;
    }
    let read_elapsed = read_started_at.elapsed();
    // Charge the shared read once; per-chunk byte and hash counts stay unchanged.
    let mut timings = Vec::with_capacity(chunks.len());
    let mut output_writer = ReconstructionOutputWriter::new(output_handles, control);
    let mut start = 0;
    for (index, chunk) in chunks.iter().enumerate() {
        if index != 0 {
            control.checkpoint()?;
        }
        let end = start + chunk.raw_size as usize;
        timings.push(verify_and_write_source_chunk(
            chunk,
            &bytes[start..end],
            if index == 0 {
                read_elapsed
            } else {
                Duration::ZERO
            },
            output_handles,
            control,
            Some(&mut output_writer),
        )?);
        start = end;
    }
    // File readiness must not precede writes still held in the private buffer.
    let flush_started_at = ProfileInstant::detailed(control.profiler());
    output_writer.flush()?;
    let flush_elapsed = flush_started_at.elapsed();
    let last = timings.last_mut().expect("source batch is nonempty");
    last.file_write += flush_elapsed;
    last.file_rebuild += flush_elapsed;
    Ok(timings)
}

fn verify_and_write_source_chunk(
    job: &ReconstructSourceChunkJob,
    bytes: &[u8],
    read_elapsed: Duration,
    output_handles: &OutputHandleCache,
    control: &OperationControl,
    mut output_writer: Option<&mut ReconstructionOutputWriter<'_>>,
) -> Result<ReconstructTimings> {
    let hash_started_at = ProfileInstant::detailed(control.profiler());
    let actual_hash = ChunkHash::of(bytes);
    let hash_elapsed = hash_started_at.elapsed();
    if actual_hash != job.chunk_hash {
        return Err(Error::HashMismatch {
            expected: job.chunk_hash.to_hex(),
            actual: actual_hash.to_hex(),
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
    for target in job.targets() {
        let write_started_at = ProfileInstant::detailed(control.profiler());
        if let Some(writer) = output_writer.as_deref_mut() {
            if job.additional_targets.is_empty() {
                writer.write(target.file_index, target.file_offset, bytes)?;
            } else {
                writer.write_unbuffered(target.file_index, target.file_offset, bytes)?;
            }
        } else {
            control.checkpoint()?;
            let output =
                cached_target_handle(output_handles, &mut target_handles, target.file_index)?;
            write_file_at(&output, target.file_offset, bytes, control).map_err(|e| {
                e.with_io_context(
                    "writing reused source range",
                    output_handles.output_path(target.file_index),
                )
            })?;
        }
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
    control: &OperationControl,
) -> Result<ReconstructTimings> {
    let file_plan = file_plans.get(file_index).ok_or_else(|| {
        Error::reconstruct(format!("source file job references file {file_index}"))
    })?;
    control.checkpoint()?;
    let _source_span = control.profile_span_with_args("installation.file.copy_and_hash", || {
        file_trace_args(&file_plan.path, file_plan.size)
    });
    let mut input = {
        File::open(source_path)
            .map_err(|e| Error::io_context("opening source file", source_path, e))?
    };
    let mut output = {
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&output_paths[file_index])
            .map_err(|e| {
                Error::io_context("opening copied output file", &output_paths[file_index], e)
            })?
    };
    let mut timings = ReconstructTimings::default();
    let mut buffer = vec![0u8; 1024 * 1024];
    let mut full_file_hasher = blake3::Hasher::new();
    let mut copied = 0u64;
    loop {
        control.checkpoint()?;
        let read = {
            input
                .read(&mut buffer)
                .map_err(|e| Error::io_context("reading source file", source_path, e))?
        };
        if read == 0 {
            break;
        }
        full_file_hasher.update(&buffer[..read]);

        let write_started_at = ProfileInstant::detailed(control.profiler());
        let mut offset = 0;
        while offset < read {
            let count = output.write(&buffer[offset..read]).map_err(|e| {
                Error::io_context("writing copied output file", &output_paths[file_index], e)
            })?;
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
        output.flush().map_err(|e| {
            Error::io_context("flushing copied output file", &output_paths[file_index], e)
        })?;
        timings.file_write += flush_started_at.elapsed();
    }
    timings.file_rebuild = timings.file_write;
    if copied != file_plan.size {
        return Err(Error::reconstruct(format!(
            "copied source file '{}' has size {}, expected {}",
            file_plan.path, copied, file_plan.size
        )));
    }
    let actual_hash = ChunkHash::from_blake3(full_file_hasher.finalize());
    if actual_hash != file_plan.full_file_hash {
        return Err(Error::HashMismatch {
            expected: file_plan.full_file_hash.to_hex(),
            actual: actual_hash.to_hex(),
        });
    }
    Ok(timings)
}

fn write_reconstruct_targets(
    chunk: &ReconstructArtifactChunk,
    targets: &[ReconstructTarget],
    bytes: &[u8],
    output_writer: &mut ReconstructionOutputWriter<'_>,
    control: &OperationControl,
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
        if targets.len() == 1 {
            output_writer.write(target.file_index, target.file_offset, bytes)?;
        } else {
            // Cross-file fanout cannot combine adjacent writes; avoid copying each target.
            output_writer.write_unbuffered(target.file_index, target.file_offset, bytes)?;
        }
        timings.file_write += write_started_at.elapsed();
        timings.file_write_bytes = timings.file_write_bytes.saturating_add(bytes.len() as u64);
        timings.file_rebuild_bytes = timings
            .file_rebuild_bytes
            .saturating_add(bytes.len() as u64);
    }
    timings.file_rebuild += rebuild_started_at.elapsed();
    Ok(())
}

fn reconstruct_artifact_trace_args(
    job: &ReconstructArtifactJob,
) -> BTreeMap<String, serde_json::Value> {
    let logical_bytes = job
        .chunks
        .iter()
        .map(|chunk| u64::from(chunk.raw_size).saturating_mul(chunk.target_range.len() as u64))
        .sum::<u64>();
    BTreeMap::from([
        (
            "artifact_kind".to_string(),
            serde_json::json!(match &job.artifact {
                ArtifactRef::Bundle { .. } => "bundle",
                ArtifactRef::LooseChunk { .. } => "loose_chunk",
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

pub(crate) struct ReconstructedFileVerification {
    pub(crate) elapsed: Duration,
    pub(crate) bytes: u64,
}

// Every reconstruction writer hash-checks each chunk (or, for whole-file
// copies, the full file) before writing it, and planning proves each file's
// chunks tile it exactly. Rereading the output to hash it again would only
// repeat that proof, so reconstruction checks just the size. Lost or damaged
// writes after that point are what explicit `verify` detects.
pub(crate) fn verify_reconstructed_output_file(
    file_plan: &ReconstructPipelineFile,
    output_path: &Path,
    control: &OperationControl,
) -> Result<ReconstructedFileVerification> {
    control.checkpoint()?;
    let started_at = ProfileInstant::detailed(control.profiler());
    let actual_size = fs::metadata(output_path)
        .map_err(|e| Error::io_context("reading reconstructed file metadata", output_path, e))?
        .len();
    if actual_size != file_plan.size {
        return Err(Error::reconstruct(format!(
            "reconstructed size {} does not match expected {} for {}",
            actual_size, file_plan.size, file_plan.path
        )));
    }
    Ok(ReconstructedFileVerification {
        elapsed: started_at.elapsed(),
        bytes: actual_size,
    })
}

pub(crate) fn build_source_chunk_locations(
    source: SnapshotReconstructSource<'_>,
    required_chunks: Option<&HashSet<ChunkHash>>,
) -> Result<HashMap<ChunkHash, SourceChunkLocation>> {
    let mut chunk_locations = HashMap::new();
    for file_manifest in source.snapshot_manifest.files() {
        let typed_file = typed_file_manifest(file_manifest)?;
        // One immutable path per file is shared by its locations and source jobs.
        let source_path = Arc::new(resolve_manifest_relative_path(
            source.destination_dir,
            &file_manifest.path,
        )?);
        let mut offset = 0u64;
        for chunk_ref in typed_file.chunks {
            let raw_size = u64::from(chunk_ref.raw_size);
            if required_chunks.is_none_or(|required| required.contains(&chunk_ref.chunk_hash)) {
                chunk_locations
                    .entry(chunk_ref.chunk_hash)
                    .or_insert_with(|| SourceChunkLocation {
                        path: source_path.clone(),
                        offset,
                        raw_size,
                    });
            }
            // Excluded chunks still occupy bytes before a later selected chunk.
            offset = offset.checked_add(raw_size).ok_or_else(|| {
                Error::manifest(format!(
                    "source file '{}' chunk sizes overflow u64",
                    file_manifest.path
                ))
            })?;
        }
    }
    Ok(chunk_locations)
}

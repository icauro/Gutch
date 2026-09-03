#[cfg(test)]
use fastcdc::v2020::{Chunk, FastCDC};
use fastcdc::v2020::{Normalization, StreamCDC};
use rayon::ThreadPool;
use rayon::prelude::*;
use std::io::BufReader;
use std::time::{Duration, Instant};

use crate::error::Result;
use crate::hash::hash_hex;
use crate::manifest::ChunkSizingProfile;

pub(crate) struct ChunkResult {
    pub(crate) data: Vec<u8>,
    #[cfg(test)]
    pub(crate) offset: u64,
    pub(crate) hash: blake3::Hash,
}

struct PendingChunk {
    data: Vec<u8>,
    #[cfg(test)]
    offset: u64,
}

pub(crate) struct ChunkFileScan {
    pub(crate) chunks: Vec<ChunkResult>,
    pub(crate) file_size: u64,
    pub(crate) full_file_hash: String,
    pub(crate) full_file_hash_duration: Duration,
}

#[cfg(test)]
struct ChunkSlice {
    offset: u64,
    len: usize,
    hash: blake3::Hash,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ChunkScanTimings {
    pub(crate) boundary_scan: Duration,
    pub(crate) boundary_scan_bytes: u64,
    pub(crate) hash_judgment: Duration,
    pub(crate) hash_judgment_bytes: u64,
}

#[cfg(test)]
pub(crate) fn chunk_bytes(data: &[u8], profile: &ChunkSizingProfile) -> Vec<ChunkResult> {
    chunk_slices(data, profile)
        .0
        .into_iter()
        .map(|chunk| ChunkResult {
            data: data[chunk.offset as usize..chunk.offset as usize + chunk.len].to_vec(),
            #[cfg(test)]
            offset: chunk.offset,
            hash: chunk.hash,
        })
        .collect()
}

#[cfg(test)]
fn chunk_slices(data: &[u8], profile: &ChunkSizingProfile) -> (Vec<ChunkSlice>, ChunkScanTimings) {
    let min = profile.min_chunk_size_bytes() as usize;
    let avg = profile.avg_chunk_size_bytes() as usize;
    let max = profile.max_chunk_size_bytes() as usize;

    let boundary_start = Instant::now();
    let chunker = FastCDC::with_level(data, min, avg, max, Normalization::Level1);
    let chunks: Vec<Chunk> = chunker.collect();
    let boundary_scan = boundary_start.elapsed();

    let hash_start = Instant::now();
    let mut chunk_slices = Vec::with_capacity(chunks.len());
    for chunk in chunks {
        chunk_slices.push(chunk_slice_from_cdc(chunk, data));
    }
    let hash_judgment = hash_start.elapsed();

    (
        chunk_slices,
        ChunkScanTimings {
            boundary_scan,
            boundary_scan_bytes: data.len() as u64,
            hash_judgment,
            hash_judgment_bytes: data.len() as u64,
        },
    )
}

#[cfg(test)]
fn chunk_slice_from_cdc(chunk: Chunk, source: &[u8]) -> ChunkSlice {
    let offset = chunk.offset;
    let length = chunk.length;
    let hash = blake3::hash(&source[offset..offset + length]);
    ChunkSlice {
        offset: offset as u64,
        len: length,
        hash,
    }
}

#[cfg(test)]
pub(crate) fn chunk_file_streaming_with_file_hash(
    path: &std::path::Path,
    profile: &ChunkSizingProfile,
) -> Result<(ChunkFileScan, ChunkScanTimings)> {
    chunk_file_streaming_with_file_hash_and_control_with_pool(
        path,
        profile,
        &crate::GutchOperationControl::default(),
        None,
    )
}

pub(crate) fn chunk_file_streaming_with_file_hash_and_control_on_pool(
    path: &std::path::Path,
    profile: &ChunkSizingProfile,
    control: &crate::GutchOperationControl,
    worker_pool: &ThreadPool,
) -> Result<(ChunkFileScan, ChunkScanTimings)> {
    chunk_file_streaming_with_file_hash_and_control_with_pool(
        path,
        profile,
        control,
        Some(worker_pool),
    )
}

fn chunk_file_streaming_with_file_hash_and_control_with_pool(
    path: &std::path::Path,
    profile: &ChunkSizingProfile,
    control: &crate::GutchOperationControl,
    worker_pool: Option<&ThreadPool>,
) -> Result<(ChunkFileScan, ChunkScanTimings)> {
    control.checkpoint()?;
    let min = profile.min_chunk_size_bytes() as usize;
    let avg = profile.avg_chunk_size_bytes() as usize;
    let max = profile.max_chunk_size_bytes() as usize;

    let file = std::fs::File::open(path)?;
    let reader = BufReader::new(file);
    let mut chunker = StreamCDC::with_level(reader, min, avg, max, Normalization::Level1);

    let mut pending_chunks = Vec::new();
    let mut boundary_scan = Duration::ZERO;
    let mut boundary_scan_bytes = 0u64;
    let mut hash_judgment = Duration::ZERO;
    let mut hash_judgment_bytes = 0u64;
    let mut full_file_hash = blake3::Hasher::new();
    let mut full_file_hash_duration = Duration::ZERO;
    let mut file_size = 0u64;
    let _cdc_span = control.profile_span_with_args(
        "capture.scan_source.file.discover_cdc_boundaries",
        std::collections::BTreeMap::from([
            ("min_chunk_bytes".to_string(), serde_json::json!(min)),
            ("avg_chunk_bytes".to_string(), serde_json::json!(avg)),
            ("max_chunk_bytes".to_string(), serde_json::json!(max)),
        ]),
    );

    loop {
        control.checkpoint()?;
        let t = Instant::now();
        let next_chunk = chunker.next();
        boundary_scan += t.elapsed();

        let Some(chunk_result) = next_chunk else {
            break;
        };
        let chunk = chunk_result.map_err(|e| crate::error::Error::Chunk(e.to_string()))?;

        let t = Instant::now();
        full_file_hash.update(&chunk.data);
        full_file_hash_duration += t.elapsed();

        file_size += chunk.length as u64;
        boundary_scan_bytes += chunk.length as u64;
        pending_chunks.push(PendingChunk {
            data: chunk.data,
            #[cfg(test)]
            offset: chunk.offset,
        });
        hash_judgment_bytes += chunk.length as u64;
    }
    drop(_cdc_span);

    let hash_worker_count = worker_pool
        .map(ThreadPool::current_num_threads)
        .unwrap_or(1);
    let mut hash_trace_args = std::collections::BTreeMap::new();
    hash_trace_args.insert(
        "chunk_count".to_string(),
        serde_json::json!(pending_chunks.len()),
    );
    hash_trace_args.insert(
        "bytes_mib".to_string(),
        serde_json::json!(file_size as f64 / (1024.0 * 1024.0)),
    );
    hash_trace_args.insert(
        "worker_count".to_string(),
        serde_json::json!(hash_worker_count),
    );
    hash_trace_args.insert(
        "parallel".to_string(),
        serde_json::json!(worker_pool.is_some() && hash_worker_count > 1),
    );
    let _hash_span =
        control.profile_span_with_args("capture.scan_source.file.hash_chunks", hash_trace_args);
    let hashed_chunks = if let Some(worker_pool) = worker_pool {
        let control_for_work = control.clone();
        worker_pool.install(|| {
            pending_chunks
                .into_par_iter()
                .map(|pending| hash_pending_chunk(pending, &control_for_work))
                .collect::<Result<Vec<_>>>()
        })?
    } else {
        pending_chunks
            .into_iter()
            .map(|pending| hash_pending_chunk(pending, control))
            .collect::<Result<Vec<_>>>()?
    };
    let mut chunk_results = Vec::with_capacity(hashed_chunks.len());
    for (chunk, elapsed) in hashed_chunks {
        hash_judgment += elapsed;
        chunk_results.push(chunk);
    }

    Ok((
        ChunkFileScan {
            chunks: chunk_results,
            file_size,
            full_file_hash: hash_hex(&full_file_hash.finalize()),
            full_file_hash_duration,
        },
        ChunkScanTimings {
            boundary_scan,
            boundary_scan_bytes,
            hash_judgment,
            hash_judgment_bytes,
        },
    ))
}

fn hash_pending_chunk(
    pending: PendingChunk,
    control: &crate::GutchOperationControl,
) -> Result<(ChunkResult, Duration)> {
    control.checkpoint()?;
    let started_at = Instant::now();
    let hash = blake3::hash(&pending.data);
    let elapsed = started_at.elapsed();
    Ok((
        ChunkResult {
            data: pending.data,
            #[cfg(test)]
            offset: pending.offset,
            hash,
        },
        elapsed,
    ))
}

#[cfg(test)]
#[path = "chunk_tests.rs"]
mod tests;

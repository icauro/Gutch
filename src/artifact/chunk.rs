#[cfg(test)]
use fastcdc::v2020::{Chunk, FastCDC};
use fastcdc::v2020::{Normalization, StreamCDC};
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
    chunk_file_streaming_with_file_hash_and_control(
        path,
        profile,
        &crate::GutchOperationControl::default(),
    )
}

pub(crate) fn chunk_file_streaming_with_file_hash_and_control(
    path: &std::path::Path,
    profile: &ChunkSizingProfile,
    control: &crate::GutchOperationControl,
) -> Result<(ChunkFileScan, ChunkScanTimings)> {
    control.checkpoint()?;
    let min = profile.min_chunk_size_bytes() as usize;
    let avg = profile.avg_chunk_size_bytes() as usize;
    let max = profile.max_chunk_size_bytes() as usize;

    let file = std::fs::File::open(path)?;
    let reader = BufReader::new(file);
    let mut chunker = StreamCDC::with_level(reader, min, avg, max, Normalization::Level1);

    let mut chunk_results = Vec::new();
    let mut boundary_scan = Duration::ZERO;
    let mut boundary_scan_bytes = 0u64;
    let mut hash_judgment = Duration::ZERO;
    let mut hash_judgment_bytes = 0u64;
    let mut full_file_hash = blake3::Hasher::new();
    let mut full_file_hash_duration = Duration::ZERO;
    let mut file_size = 0u64;

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
        let hash = blake3::hash(&chunk.data);
        hash_judgment += t.elapsed();

        let t = Instant::now();
        full_file_hash.update(&chunk.data);
        full_file_hash_duration += t.elapsed();

        file_size += chunk.length as u64;
        boundary_scan_bytes += chunk.length as u64;
        hash_judgment_bytes += chunk.length as u64;
        chunk_results.push(ChunkResult {
            data: chunk.data,
            #[cfg(test)]
            offset: chunk.offset,
            hash,
        });
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

#[cfg(test)]
#[path = "chunk_tests.rs"]
mod tests;

//! Content-defined chunking of files and byte slices with the selected chunker.

use super::chunk_stream::ChunkBoundary;
use super::chunk_stream::{BoundaryFinder, ChunkStream, KeyedBoundaryFinder};
use super::gear::GearTables;
use crate::profiling::ProfileInstant;
#[cfg(test)]
use fastcdc::v2020::Chunk;
#[cfg(test)]
use fastcdc::v2020::FastCDC;
#[cfg(test)]
use fastcdc::v2020::Normalization;
use std::io::Read;
use std::time::Duration;

use crate::artifact::hash::ChunkHash;
use crate::artifact::profiles::ChunkSizingProfile;
use crate::artifact::profiles::Chunker;
use crate::error::Result;
use crate::progress::OperationControl;
use std::fs::File;
use std::path::Path;

pub(crate) struct ChunkResult {
    pub(crate) size: u64,
    pub(crate) offset: u64,
    pub(crate) hash: ChunkHash,
}

pub(crate) struct ChunkFileScan {
    pub(crate) chunks: Vec<ChunkResult>,
    pub(crate) file_size: u64,
    pub(crate) full_file_hash: ChunkHash,
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
            size: chunk.len as u64,
            offset: chunk.offset,
            hash: ChunkHash::from_blake3(chunk.hash),
        })
        .collect()
}

#[cfg(test)]
fn chunk_slices(data: &[u8], profile: &ChunkSizingProfile) -> (Vec<ChunkSlice>, ChunkScanTimings) {
    let min = profile.min_chunk_bytes as usize;
    let avg = profile.avg_chunk_bytes as usize;
    let max = profile.max_chunk_bytes as usize;

    let boundary_start = ProfileInstant::detailed(None);
    let chunker = FastCDC::with_level(data, min, avg, max, Normalization::Level1);
    let chunks: Vec<Chunk> = chunker.collect();
    let boundary_scan = boundary_start.elapsed();

    let hash_start = ProfileInstant::detailed(None);
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

/// Chunks a file. `gear` holds an encrypted repository's keyed tables, which
/// only the FastCDC chunker can use.
pub(crate) fn scan_file_chunks(
    path: &Path,
    inventory_size: u64,
    profile: &ChunkSizingProfile,
    gear: Option<&GearTables>,
    control: &OperationControl,
) -> Result<(ChunkFileScan, ChunkScanTimings)> {
    if let Some(tables) = gear {
        check_keyed_chunker(profile.chunker)?;
        let finder = KeyedBoundaryFinder::new(profile, tables);
        return scan_with_boundary(path, inventory_size, control, finder);
    }
    match profile.chunker {
        Chunker::FastCdc => {
            scan_with_boundary(path, inventory_size, control, BoundaryFinder::new(profile))
        }
        Chunker::Vram256 => {
            let scalar = super::ram::Ram::new(
                profile.avg_chunk_bytes as usize,
                profile.max_chunk_bytes as usize,
            );
            if let Some(vector) = scalar.accelerated() {
                scan_with_boundary(path, inventory_size, control, vector)
            } else {
                scan_with_boundary(path, inventory_size, control, scalar)
            }
        }
        Chunker::Jc => scan_with_boundary(
            path,
            inventory_size,
            control,
            super::jc::JumpChunker::new(
                profile.min_chunk_bytes as usize,
                profile.avg_chunk_bytes as usize,
                profile.max_chunk_bytes as usize,
                super::jc::Parameters::Plakar,
            ),
        ),
    }
}

/// Encrypted repositories chunk with keyed FastCDC; the other chunkers have
/// no key and would reveal content through chunk sizes.
pub(crate) fn check_keyed_chunker(chunker: Chunker) -> Result<()> {
    match chunker {
        Chunker::FastCdc => Ok(()),
        _ => Err(crate::Error::invalid_input(
            "encrypted repositories chunk with FastCDC; choose the fastcdc chunker",
        )),
    }
}

fn scan_with_boundary<C: ChunkBoundary>(
    path: &Path,
    inventory_size: u64,
    control: &OperationControl,
    boundary: C,
) -> Result<(ChunkFileScan, ChunkScanTimings)> {
    control.checkpoint()?;
    let max = boundary.max_size();

    let file = File::open(path)?;
    // The inventory already knows the size. Keep this path within the streaming
    // chunker's buffer bound, without another metadata query or a probing pass.
    let small_file_limit = (max as u64).min(1024 * 1024);
    if inventory_size <= small_file_limit {
        return scan_small_file(file, inventory_size, control, boundary);
    }
    let mut chunker = ChunkStream::with_boundary(file, boundary, inventory_size);

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
        let boundary_scan_started_at = ProfileInstant::detailed(control.profiler());
        let next_chunk = chunker.next(control);
        boundary_scan += boundary_scan_started_at.elapsed();

        let Some(chunk) = next_chunk? else {
            break;
        };

        let file_hash_started_at = ProfileInstant::detailed(control.profiler());
        full_file_hash.update(chunk.data);
        full_file_hash_duration += file_hash_started_at.elapsed();

        file_size += chunk.data.len() as u64;
        boundary_scan_bytes += chunk.data.len() as u64;
        let started_at = ProfileInstant::detailed(control.profiler());
        let hash = ChunkHash::of(chunk.data);
        hash_judgment += started_at.elapsed();
        chunk_results.push(ChunkResult {
            offset: chunk.offset,
            size: chunk.data.len() as u64,
            hash,
        });
        hash_judgment_bytes += chunk.data.len() as u64;
    }
    // Match the small-file path: a source that grew or shrank while it was read
    // would otherwise be recorded with a size that differs from its inventory.
    if file_size != inventory_size {
        return Err(crate::Error::SourceChanged { path: None });
    }

    Ok((
        ChunkFileScan {
            chunks: chunk_results,
            file_size,
            full_file_hash: ChunkHash::from_blake3(full_file_hash.finalize()),
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

fn scan_small_file<C: ChunkBoundary>(
    file: File,
    inventory_size: u64,
    control: &OperationControl,
    boundary: C,
) -> Result<(ChunkFileScan, ChunkScanTimings)> {
    let started = ProfileInstant::detailed(control.profiler());
    let mut bytes = Vec::with_capacity(inventory_size as usize + 1);
    // The extra byte detects growth without letting a changing source turn a
    // bounded small-file read into an unbounded allocation.
    file.take(inventory_size + 1).read_to_end(&mut bytes)?;
    control.checkpoint()?;
    if bytes.len() as u64 != inventory_size {
        return Err(crate::Error::SourceChanged { path: None });
    }
    let mut boundary_scan = started.elapsed();
    let started = ProfileInstant::detailed(control.profiler());
    let full_hash = blake3::hash(&bytes);
    let full_file_hash_duration = started.elapsed();
    let mut offset = 0;
    let mut chunks = Vec::new();
    let mut hash_judgment = Duration::ZERO;
    loop {
        control.checkpoint()?;
        let started = ProfileInstant::detailed(control.profiler());
        if offset == bytes.len() {
            break;
        }
        let length = boundary.cut(&bytes[offset..]);
        boundary_scan += started.elapsed();
        let started = ProfileInstant::detailed(control.profiler());
        let hash = ChunkHash::from_blake3(if length == bytes.len() {
            full_hash
        } else {
            blake3::hash(&bytes[offset..offset + length])
        });
        hash_judgment += started.elapsed();
        chunks.push(ChunkResult {
            offset: offset as u64,
            size: length as u64,
            hash,
        });
        offset += length;
    }
    Ok((
        ChunkFileScan {
            chunks,
            file_size: inventory_size,
            full_file_hash: ChunkHash::from_blake3(full_hash),
            full_file_hash_duration,
        },
        ChunkScanTimings {
            boundary_scan,
            boundary_scan_bytes: inventory_size,
            hash_judgment,
            hash_judgment_bytes: inventory_size,
        },
    ))
}

#[cfg(test)]
#[path = "chunk_tests.rs"]
mod tests;

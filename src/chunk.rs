use fastcdc::v2020::{Chunk, FastCDC, Normalization};
use std::time::{Duration, Instant};

use crate::error::Result;
use crate::manifest::ChunkSizingProfile;

pub struct ChunkResult {
    pub data: Vec<u8>,
    pub offset: u64,
    pub hash: blake3::Hash,
}

pub struct ChunkSlice {
    pub offset: u64,
    pub len: usize,
    pub hash: blake3::Hash,
}

#[derive(Debug, Clone, Default)]
pub struct ChunkScanTimings {
    pub boundary_scan: Duration,
    pub boundary_scan_bytes: u64,
    pub hash_judgment: Duration,
    pub hash_judgment_bytes: u64,
}

pub fn chunk_bytes(data: &[u8], profile: &ChunkSizingProfile) -> Vec<ChunkResult> {
    chunk_slices(data, profile)
        .0
        .into_iter()
        .map(|chunk| ChunkResult {
            data: data[chunk.offset as usize..chunk.offset as usize + chunk.len].to_vec(),
            offset: chunk.offset,
            hash: chunk.hash,
        })
        .collect()
}

pub fn chunk_slices(
    data: &[u8],
    profile: &ChunkSizingProfile,
) -> (Vec<ChunkSlice>, ChunkScanTimings) {
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

fn chunk_slice_from_cdc(chunk: Chunk, source: &[u8]) -> ChunkSlice {
    let offset: usize = chunk.offset as usize;
    let length: usize = chunk.length as usize;
    let hash = blake3::hash(&source[offset..offset + length]);
    ChunkSlice {
        offset: offset as u64,
        len: length,
        hash,
    }
}

pub fn chunk_file(
    path: &std::path::Path,
    profile: &ChunkSizingProfile,
) -> Result<Vec<ChunkResult>> {
    let data = std::fs::read(path)?;
    Ok(chunk_bytes(&data, profile))
}

pub fn hash_hex(hash: &blake3::Hash) -> String {
    hash.to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::ChunkSizingProfile;

    #[test]
    fn test_chunk_bytes_basic() {
        let data = b"hello world here is some test data that should be longer than the minimum chunk size ".repeat(1000);
        let profile = ChunkSizingProfile::default();
        let results = chunk_bytes(&data, &profile);
        assert!(!results.is_empty());
        for r in &results {
            assert!(!r.data.is_empty());
            assert!(r.data.len() as u32 <= profile.max_chunk_size_bytes());
        }
    }

    #[test]
    fn test_chunk_bytes_empty() {
        let data: &[u8] = &[];
        let profile = ChunkSizingProfile::default();
        let results = chunk_bytes(data, &profile);
        assert!(results.is_empty());
    }

    #[test]
    fn test_chunk_bytes_small_data() {
        let data = b"short";
        let profile = ChunkSizingProfile::default();
        let results = chunk_bytes(data, &profile);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].data, b"short");
    }

    #[test]
    fn test_hash_hex() {
        let hash = blake3::hash(b"test");
        let hex = hash_hex(&hash);
        assert_eq!(hex.len(), 64);
        assert!(hex.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_chunks_have_increasing_offsets() {
        let data = b"x".repeat(1024 * 64);
        let profile = ChunkSizingProfile::default();
        let results = chunk_bytes(&data, &profile);
        for w in results.windows(2) {
            assert!(w[1].offset > w[0].offset);
        }
    }
}

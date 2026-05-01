use fastcdc::v2020::{ChunkData, StreamCDC};
use crate::error::Result;
use crate::manifest::ChunkingProfile;

pub struct ChunkResult {
    pub data: Vec<u8>,
    pub offset: u64,
    pub hash: blake3::Hash,
}

pub fn chunk_bytes(data: &[u8], profile: &ChunkingProfile) -> Vec<ChunkResult> {
    let min = profile.min_chunk_size as usize;
    let avg = profile.avg_chunk_size as usize;
    let max = profile.max_chunk_size as usize;

    let chunker = StreamCDC::new(data, min, avg, max);
    chunker
        .filter_map(|res| res.ok().map(|chunk| chunk_from_cdc(chunk, data)))
        .collect()
}

fn chunk_from_cdc(chunk: ChunkData, source: &[u8]) -> ChunkResult {
    let offset: usize = chunk.offset as usize;
    let length: usize = chunk.length as usize;
    let data = source[offset..offset + length].to_vec();
    let hash = blake3::hash(&data);
    ChunkResult {
        data,
        offset: offset as u64,
        hash,
    }
}

pub fn chunk_file(path: &std::path::Path, profile: &ChunkingProfile) -> Result<Vec<ChunkResult>> {
    let data = std::fs::read(path)?;
    Ok(chunk_bytes(&data, profile))
}

pub fn hash_hex(hash: &blake3::Hash) -> String {
    hash.to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::ChunkingProfile;

    #[test]
    fn test_chunk_bytes_basic() {
        let data = b"hello world here is some test data that should be longer than the minimum chunk size ".repeat(1000);
        let profile = ChunkingProfile::default();
        let results = chunk_bytes(&data, &profile);
        assert!(!results.is_empty());
        for r in &results {
            assert!(!r.data.is_empty());
            assert!(r.data.len() as u32 <= profile.max_chunk_size);
        }
    }

    #[test]
    fn test_chunk_bytes_empty() {
        let data: &[u8] = &[];
        let profile = ChunkingProfile::default();
        let results = chunk_bytes(data, &profile);
        assert!(results.is_empty());
    }

    #[test]
    fn test_chunk_bytes_small_data() {
        let data = b"short";
        let profile = ChunkingProfile::default();
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
        let profile = ChunkingProfile::default();
        let results = chunk_bytes(&data, &profile);
        for w in results.windows(2) {
            assert!(w[1].offset > w[0].offset);
        }
    }
}
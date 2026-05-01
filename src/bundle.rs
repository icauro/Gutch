use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};

use crate::compress;
use crate::error::Result;
use crate::hash;
use crate::manifest::{BundleTableEntry, CompressionProfile};

const MAGIC: &[u8; 4] = b"GCHB";
const FORMAT_VERSION: u8 = 1;

pub struct BundleBuilder {
    chunks: Vec<ChunkData>,
    pub profile: CompressionProfile,
}

struct ChunkData {
    hash: String,
    raw: Vec<u8>,
}

impl BundleBuilder {
    pub fn new(profile: CompressionProfile) -> Self {
        BundleBuilder { chunks: Vec::new(), profile }
    }

    pub fn add_chunk(&mut self, chunk_hash: String, raw_data: Vec<u8>) {
        self.chunks.push(ChunkData { hash: chunk_hash, raw: raw_data });
    }

    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }

    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    pub fn compute_bundle_id(&self) -> String {
        let mut hashes: Vec<&str> = self.chunks.iter().map(|c| c.hash.as_str()).collect();
        hashes.sort();

        let mut hasher = blake3::Hasher::new();
        for h in &hashes {
            hasher.update(h.as_bytes());
            hasher.update(b"\x00");
        }
        hasher.update(&FORMAT_VERSION.to_le_bytes());
        hash::hash_hex(&hasher.finalize())
    }

    pub fn estimated_compressed_size(&self) -> u64 {
        self.chunks.iter().map(|c| {
            let compressed: std::result::Result<Vec<u8>, _> = compress::compress_with_level(&c.raw, self.profile.zstd_level);
            compressed.map(|v: Vec<u8>| v.len() as u64).unwrap_or(u64::MAX)
        }).sum::<u64>() + self.estimate_overhead()
    }

    fn estimate_overhead(&self) -> u64 {
        8 + (self.chunks.len() as u64 * 128) + 36
    }

    pub fn build(&self) -> Result<Vec<u8>> {
        let mut compressed_entries: Vec<(BundleTableEntry, Vec<u8>)> = Vec::new();
        let mut current_offset: u64 = 0;

        for chunk in &self.chunks {
            let compressed_data = compress::compress_with_level(&chunk.raw, self.profile.zstd_level)?;
            let compressed_size = compressed_data.len() as u64;
            let entry = BundleTableEntry {
                chunk_hash: chunk.hash.clone(),
                raw_size: chunk.raw.len() as u64,
                compressed_size,
                compressed_offset: current_offset,
                compression: "zstd".to_string(),
            };
            compressed_entries.push((entry, compressed_data));
            current_offset += compressed_size;
        }

        let chunk_count = compressed_entries.len() as u32;
        let mut header = Vec::new();
        header.extend_from_slice(MAGIC);
        header.push(FORMAT_VERSION);
        header.extend_from_slice(&chunk_count.to_le_bytes());
        header.extend_from_slice(&0u32.to_le_bytes());

        let table_json = serde_json::to_string(&compressed_entries.iter().map(|(e, _)| e).collect::<Vec<_>>())?;
        header.extend_from_slice(&(table_json.len() as u32).to_le_bytes());
        header.extend_from_slice(table_json.as_bytes());

        let mut bundle = header;
        for (_, data) in &compressed_entries {
            bundle.extend_from_slice(data);
        }

        let bundle_hash = self.compute_bundle_id();
        bundle.extend_from_slice(bundle_hash.as_bytes());

        Ok(bundle)
    }
}

pub struct BundleReader<R: Read + Seek> {
    entries: Vec<BundleTableEntry>,
    data_start: u64,
    reader: R,
}

impl<R: Read + Seek> BundleReader<R> {
    pub fn open(mut reader: R) -> Result<Self> {
        let mut magic = [0u8; 4];
        reader.read_exact(&mut magic)?;
        if &magic != MAGIC {
            return Err(crate::error::Error::InvalidBundle("invalid magic bytes".into()));
        }

        let mut version = [0u8; 1];
        reader.read_exact(&mut version)?;
        if version[0] != FORMAT_VERSION {
            return Err(crate::error::Error::InvalidBundle(format!(
                "unsupported version: {}",
                version[0]
            )));
        }

        let mut count_buf = [0u8; 4];
        reader.read_exact(&mut count_buf)?;
        let _chunk_count = u32::from_le_bytes(count_buf);

        reader.read_exact(&mut count_buf)?;

        reader.read_exact(&mut count_buf)?;
        let table_len = u32::from_le_bytes(count_buf) as usize;

        let mut table_buf = vec![0u8; table_len];
        reader.read_exact(&mut table_buf)?;
        let entries: Vec<BundleTableEntry> = serde_json::from_slice(&table_buf)?;

        let data_start = reader.stream_position()?;

        Ok(BundleReader { entries, data_start, reader })
    }

    pub fn chunk_count(&self) -> usize {
        self.entries.len()
    }

    pub fn entries(&self) -> &[BundleTableEntry] {
        &self.entries
    }

    pub fn chunk_index(&self) -> HashMap<String, usize> {
        self.entries.iter().enumerate().map(|(i, e)| (e.chunk_hash.clone(), i)).collect()
    }

    pub fn read_chunk(&mut self, index: usize) -> Result<Vec<u8>> {
        if index >= self.entries.len() {
            return Err(crate::error::Error::InvalidBundle(format!(
                "chunk index {} out of range ({} chunks)",
                index,
                self.entries.len()
            )));
        }

        let entry = &self.entries[index];
        self.reader.seek(SeekFrom::Start(self.data_start + entry.compressed_offset))?;

        let mut compressed = vec![0u8; entry.compressed_size as usize];
        self.reader.read_exact(&mut compressed)?;

        let decompressed = compress::decompress_chunk(&compressed)?;

        let actual_hash = hash::hash_chunk(&decompressed);
        let actual_hex = hash::hash_hex(&actual_hash);
        if actual_hex != entry.chunk_hash {
            return Err(crate::error::Error::HashMismatch {
                expected: entry.chunk_hash.clone(),
                actual: actual_hex,
            });
        }

        Ok(decompressed)
    }

    pub fn read_chunk_by_hash(&mut self, hash: &str) -> Result<Vec<u8>> {
        let index = self.entries.iter()
            .position(|e| e.chunk_hash == hash)
            .ok_or_else(|| crate::error::Error::ChunkNotFound(hash.to_string()))?;
        self.read_chunk(index)
    }

    pub fn verify(&mut self) -> Result<bool> {
        for i in 0..self.entries.len() {
            match self.read_chunk(i) {
                Ok(_) => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(true)
    }
}

pub fn read_bundle(data: &[u8]) -> Result<BundleReader<std::io::Cursor<Vec<u8>>>>
{
    let cursor = std::io::Cursor::new(data.to_vec());
    BundleReader::open(cursor)
}

pub fn bundle_id_from_chunks(chunk_hashes: &[(&str, u64)], format_version: u8) -> String {
    let mut sorted: Vec<&str> = chunk_hashes.iter().map(|(h, _)| *h).collect();
    sorted.sort();

    let mut hasher = blake3::Hasher::new();
    for h in &sorted {
        hasher.update(h.as_bytes());
        hasher.update(b"\x00");
    }
    hasher.update(&format_version.to_le_bytes());
    hash::hash_hex(&hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash;

    #[test]
    fn test_build_and_read_bundle() {
        let chunk1_data = b"chunk one data for testing ".repeat(10);
        let chunk2_data = b"chunk two different data ".repeat(10);

        let hash1 = hash::hash_hex(&hash::hash_chunk(&chunk1_data));
        let hash2 = hash::hash_hex(&hash::hash_chunk(&chunk2_data));

        let mut builder = BundleBuilder::new(CompressionProfile::default());
        builder.add_chunk(hash1.clone(), chunk1_data.to_vec());
        builder.add_chunk(hash2.clone(), chunk2_data.to_vec());

        let _bundle_id = builder.compute_bundle_id();
        let bundle_data = builder.build().unwrap();

        let mut reader = read_bundle(&bundle_data).unwrap();
        assert_eq!(reader.chunk_count(), 2);

        let read1 = reader.read_chunk_by_hash(&hash1).unwrap();
        let read2 = reader.read_chunk_by_hash(&hash2).unwrap();

        assert_eq!(read1, chunk1_data.to_vec());
        assert_eq!(read2, chunk2_data.to_vec());
    }

    #[test]
    fn test_empty_bundle() {
        let builder = BundleBuilder::new(CompressionProfile::default());
        assert!(builder.is_empty());
        assert_eq!(builder.chunk_count(), 0);
        let bundle_data = builder.build().unwrap();
        let reader = read_bundle(&bundle_data).unwrap();
        assert_eq!(reader.chunk_count(), 0);
    }

    #[test]
    fn test_single_chunk_bundle() {
        let data = b"single chunk test data ".repeat(20);
        let hash = hash::hash_hex(&hash::hash_chunk(&data));

        let mut builder = BundleBuilder::new(CompressionProfile::default());
        builder.add_chunk(hash.clone(), data.to_vec());

        let bundle_data = builder.build().unwrap();
        let mut reader = read_bundle(&bundle_data).unwrap();
        assert_eq!(reader.chunk_count(), 1);

        let result = reader.read_chunk(0).unwrap();
        assert_eq!(result, data.to_vec());
    }

    #[test]
    fn test_verify_bundle() {
        let chunk1 = b"verify test chunk one ".repeat(10);
        let chunk2 = b"verify test chunk two ".repeat(10);

        let mut builder = BundleBuilder::new(CompressionProfile::default());
        builder.add_chunk(
            hash::hash_hex(&hash::hash_chunk(&chunk1)),
            chunk1.to_vec(),
        );
        builder.add_chunk(
            hash::hash_hex(&hash::hash_chunk(&chunk2)),
            chunk2.to_vec(),
        );

        let bundle_data = builder.build().unwrap();
        let mut reader = read_bundle(&bundle_data).unwrap();
        assert!(reader.verify().unwrap());
    }

    #[test]
    fn test_bundle_id_deterministic() {
        let data1 = b"deterministic test ".repeat(15);
        let hash1 = hash::hash_hex(&hash::hash_chunk(&data1));

        let mut b1 = BundleBuilder::new(CompressionProfile::default());
        b1.add_chunk(hash1.clone(), data1.to_vec());
        let id1 = b1.compute_bundle_id();

        let mut b2 = BundleBuilder::new(CompressionProfile::default());
        b2.add_chunk(hash1, data1.to_vec());
        let id2 = b2.compute_bundle_id();

        assert_eq!(id1, id2);
    }

    #[test]
    fn test_bundle_id_different_order() {
        let d1 = b"chunk a ".repeat(10);
        let d2 = b"chunk b ".repeat(10);
        let h1 = hash::hash_hex(&hash::hash_chunk(&d1));
        let h2 = hash::hash_hex(&hash::hash_chunk(&d2));

        let mut b1 = BundleBuilder::new(CompressionProfile::default());
        b1.add_chunk(h1.clone(), d1.to_vec());
        b1.add_chunk(h2.clone(), d2.to_vec());
        let id1 = b1.compute_bundle_id();

        let mut b2 = BundleBuilder::new(CompressionProfile::default());
        b2.add_chunk(h2, d2.to_vec());
        b2.add_chunk(h1, d1.to_vec());
        let id2 = b2.compute_bundle_id();

        assert_eq!(id1, id2, "bundle ID should be independent of chunk insertion order");
    }

    #[test]
    fn test_invalid_magic() {
        let result = read_bundle(b"XXXX extra data to read");
        assert!(
            matches!(result, Err(crate::error::Error::InvalidBundle(_))),
            "should reject invalid magic"
        );
    }
}
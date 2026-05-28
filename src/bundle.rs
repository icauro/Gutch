use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};

use crate::compress;
use crate::error::Result;
use crate::hash;
use crate::manifest::{
    BundleTableEntry, COMPRESSION_UNIT_BUNDLE, COMPRESSION_UNIT_CHUNK, CompressionProfile,
};

const MAGIC: &[u8; 4] = b"GCHB";
const FORMAT_VERSION: u8 = 1;

pub struct BundleBuilder {
    chunks: Vec<ChunkData>,
    pub profile: CompressionProfile,
}

struct ChunkData {
    hash: String,
    raw_size: u64,
    raw: Option<Vec<u8>>,
    compressed: Option<Vec<u8>>,
}

impl BundleBuilder {
    pub fn new(profile: CompressionProfile) -> Self {
        BundleBuilder {
            chunks: Vec::new(),
            profile,
        }
    }

    pub fn add_chunk(&mut self, chunk_hash: String, raw_data: Vec<u8>) {
        let raw_size = raw_data.len() as u64;
        self.chunks.push(ChunkData {
            hash: chunk_hash,
            raw_size,
            raw: Some(raw_data),
            compressed: None,
        });
    }

    pub fn add_precompressed_chunk(
        &mut self,
        chunk_hash: String,
        raw_size: u64,
        compressed_data: Vec<u8>,
    ) {
        self.chunks.push(ChunkData {
            hash: chunk_hash,
            raw_size,
            raw: None,
            compressed: Some(compressed_data),
        });
    }

    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }

    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    pub fn compute_bundle_id(&self) -> String {
        let mut chunks: Vec<(&str, u64)> = self
            .chunks
            .iter()
            .map(|c| (c.hash.as_str(), c.raw_size))
            .collect();
        chunks.sort_by(|a, b| a.0.cmp(b.0).then(a.1.cmp(&b.1)));

        let mut hasher = blake3::Hasher::new();
        for (h, size) in &chunks {
            hasher.update(h.as_bytes());
            hasher.update(b"\x00");
            hasher.update(&size.to_le_bytes());
        }
        hasher.update(self.profile.compression_unit.as_bytes());
        hasher.update(b"\x00");
        hasher.update(&self.profile.zstd_level.to_le_bytes());
        hasher.update(&FORMAT_VERSION.to_le_bytes());
        hash::hash_hex(&hasher.finalize())
    }

    pub fn estimated_compressed_size(&self) -> u64 {
        self.chunks
            .iter()
            .map(|c| {
                if let Some(compressed) = &c.compressed {
                    compressed.len() as u64
                } else if let Some(raw) = &c.raw {
                    let compressed: std::result::Result<Vec<u8>, _> =
                        compress::compress_with_level(raw, self.profile.zstd_level);
                    compressed
                        .map(|v: Vec<u8>| v.len() as u64)
                        .unwrap_or(u64::MAX)
                } else {
                    u64::MAX
                }
            })
            .sum::<u64>()
            + self.estimate_overhead()
    }

    fn estimate_overhead(&self) -> u64 {
        8 + (self.chunks.len() as u64 * 128) + 36
    }

    pub fn build(&self) -> Result<Vec<u8>> {
        if self.profile.compression_unit == COMPRESSION_UNIT_BUNDLE {
            return self.build_bundle_compressed();
        }

        self.build_chunk_compressed()
    }

    fn build_chunk_compressed(&self) -> Result<Vec<u8>> {
        let mut compressed_entries: Vec<(BundleTableEntry, Vec<u8>)> = Vec::new();
        let mut current_offset: u64 = 0;

        for chunk in &self.chunks {
            let compressed_data = if let Some(compressed) = &chunk.compressed {
                compressed.clone()
            } else {
                let raw = chunk.raw.as_ref().ok_or_else(|| {
                    crate::error::Error::InvalidBundle(
                        "chunk is missing raw and compressed bytes".into(),
                    )
                })?;
                compress::compress_with_level(raw, self.profile.zstd_level)?
            };
            let compressed_size = compressed_data.len() as u64;
            let entry = BundleTableEntry {
                chunk_hash: chunk.hash.clone(),
                raw_size: chunk.raw_size,
                compressed_size,
                compressed_offset: current_offset,
                compression: "zstd".to_string(),
                compression_unit: COMPRESSION_UNIT_CHUNK.to_string(),
                raw_offset: 0,
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

        let table_json = serde_json::to_string(
            &compressed_entries
                .iter()
                .map(|(e, _)| e)
                .collect::<Vec<_>>(),
        )?;
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

    fn build_bundle_compressed(&self) -> Result<Vec<u8>> {
        let mut entries: Vec<BundleTableEntry> = Vec::with_capacity(self.chunks.len());
        let mut payload = Vec::new();
        let mut current_raw_offset: u64 = 0;

        for chunk in &self.chunks {
            let raw = chunk.raw.as_ref().ok_or_else(|| {
                crate::error::Error::InvalidBundle(
                    "bundle-compressed chunks must retain raw bytes until bundle build".into(),
                )
            })?;

            entries.push(BundleTableEntry {
                chunk_hash: chunk.hash.clone(),
                raw_size: chunk.raw_size,
                compressed_size: 0,
                compressed_offset: 0,
                compression: "zstd".to_string(),
                compression_unit: COMPRESSION_UNIT_BUNDLE.to_string(),
                raw_offset: current_raw_offset,
            });
            current_raw_offset += chunk.raw_size;
            payload.extend_from_slice(raw);
        }

        let compressed_payload = compress::compress_with_level(&payload, self.profile.zstd_level)?;
        if !entries.is_empty() {
            let total_raw = current_raw_offset.max(1);
            let total_compressed = compressed_payload.len() as u64;
            let mut assigned: u64 = 0;
            let entry_count = entries.len();
            for (index, entry) in entries.iter_mut().enumerate() {
                let remaining = total_compressed.saturating_sub(assigned);
                let share = if index + 1 == entry_count {
                    remaining
                } else {
                    ((entry.raw_size as u128 * total_compressed as u128) / total_raw as u128) as u64
                };
                entry.compressed_size = share;
                entry.compressed_offset = 0;
                assigned = assigned.saturating_add(share);
            }
        }

        let chunk_count = entries.len() as u32;
        let mut header = Vec::new();
        header.extend_from_slice(MAGIC);
        header.push(FORMAT_VERSION);
        header.extend_from_slice(&chunk_count.to_le_bytes());
        header.extend_from_slice(&0u32.to_le_bytes());

        let table_json = serde_json::to_string(&entries)?;
        header.extend_from_slice(&(table_json.len() as u32).to_le_bytes());
        header.extend_from_slice(table_json.as_bytes());

        let mut bundle = header;
        bundle.extend_from_slice(&compressed_payload);

        let bundle_hash = self.compute_bundle_id();
        bundle.extend_from_slice(bundle_hash.as_bytes());

        Ok(bundle)
    }
}

pub struct BundleReader<R: Read + Seek> {
    entries: Vec<BundleTableEntry>,
    data_start: u64,
    payload_size: u64,
    compression_unit: String,
    bundle_raw_cache: Option<Vec<u8>>,
    reader: R,
}

impl<R: Read + Seek> BundleReader<R> {
    pub fn open(mut reader: R) -> Result<Self> {
        let mut magic = [0u8; 4];
        reader.read_exact(&mut magic)?;
        if &magic != MAGIC {
            return Err(crate::error::Error::InvalidBundle(
                "invalid magic bytes".into(),
            ));
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
        let end = reader.seek(SeekFrom::End(0))?;
        let payload_size = end.saturating_sub(data_start).saturating_sub(64);
        reader.seek(SeekFrom::Start(data_start))?;
        let compression_unit = entries
            .first()
            .map(|entry| entry.compression_unit.clone())
            .unwrap_or_else(|| COMPRESSION_UNIT_CHUNK.to_string());

        Ok(BundleReader {
            entries,
            data_start,
            payload_size,
            compression_unit,
            bundle_raw_cache: None,
            reader,
        })
    }

    pub fn chunk_count(&self) -> usize {
        self.entries.len()
    }

    pub fn entries(&self) -> &[BundleTableEntry] {
        &self.entries
    }

    pub fn chunk_index(&self) -> HashMap<String, usize> {
        self.entries
            .iter()
            .enumerate()
            .map(|(i, e)| (e.chunk_hash.clone(), i))
            .collect()
    }

    pub fn compression_unit(&self) -> &str {
        &self.compression_unit
    }

    pub fn payload_size(&self) -> u64 {
        self.payload_size
    }

    pub fn stage_bundle_payload_to<W: Write>(&mut self, writer: &mut W) -> Result<u64> {
        if self.compression_unit != COMPRESSION_UNIT_BUNDLE {
            return Err(crate::error::Error::InvalidBundle(
                "bundle payload staging only applies to bundle-compressed bundles".into(),
            ));
        }

        self.reader.seek(SeekFrom::Start(self.data_start))?;
        let mut compressed = vec![0u8; self.payload_size as usize];
        self.reader.read_exact(&mut compressed)?;
        let written = compress::decompress_chunk_to_writer(&compressed, writer)?;
        Ok(written)
    }

    pub fn read_chunk(&mut self, index: usize) -> Result<Vec<u8>> {
        if index >= self.entries.len() {
            return Err(crate::error::Error::InvalidBundle(format!(
                "chunk index {} out of range ({} chunks)",
                index,
                self.entries.len()
            )));
        }

        if self.compression_unit == COMPRESSION_UNIT_BUNDLE {
            return self.read_chunk_from_bundle_stream(index);
        }

        let entry = &self.entries[index];
        self.reader
            .seek(SeekFrom::Start(self.data_start + entry.compressed_offset))?;

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

    fn read_chunk_from_bundle_stream(&mut self, index: usize) -> Result<Vec<u8>> {
        if self.bundle_raw_cache.is_none() {
            self.reader.seek(SeekFrom::Start(self.data_start))?;
            let mut compressed = vec![0u8; self.payload_size as usize];
            self.reader.read_exact(&mut compressed)?;
            let decompressed = compress::decompress_chunk(&compressed)?;
            self.bundle_raw_cache = Some(decompressed);
        }

        let entry = &self.entries[index];
        let raw = self.bundle_raw_cache.as_ref().ok_or_else(|| {
            crate::error::Error::InvalidBundle(
                "bundle payload cache missing after decompression".into(),
            )
        })?;
        let start = entry.raw_offset as usize;
        let end = start.saturating_add(entry.raw_size as usize);
        if end > raw.len() {
            return Err(crate::error::Error::InvalidBundle(format!(
                "bundle raw span {}..{} exceeds payload size {}",
                start,
                end,
                raw.len()
            )));
        }

        let decompressed = raw[start..end].to_vec();
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
        let index = self
            .entries
            .iter()
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

pub fn read_bundle(data: &[u8]) -> Result<BundleReader<std::io::Cursor<Vec<u8>>>> {
    let cursor = std::io::Cursor::new(data.to_vec());
    BundleReader::open(cursor)
}

pub fn bundle_id_from_chunks(chunk_hashes: &[(&str, u64)], format_version: u8) -> String {
    let mut sorted: Vec<(&str, u64)> = chunk_hashes.to_vec();
    sorted.sort_by(|a, b| a.0.cmp(b.0).then(a.1.cmp(&b.1)));

    let mut hasher = blake3::Hasher::new();
    for (h, size) in &sorted {
        hasher.update(h.as_bytes());
        hasher.update(b"\x00");
        hasher.update(&size.to_le_bytes());
    }
    hasher.update(&format_version.to_le_bytes());
    hash::hash_hex(&hasher.finalize())
}

pub fn bundle_id_prefix(bundle_id: &str) -> &str {
    &bundle_id[..2.min(bundle_id.len())]
}

#[cfg(test)]
#[path = "bundle_tests.rs"]
mod tests;

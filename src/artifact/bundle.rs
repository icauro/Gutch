use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::Arc;

use crate::compress;
use crate::error::Result;
use crate::hash::{self, ChunkHash};
use crate::manifest::{BundleTableEntry, CompressionKind, CompressionProfile, CompressionUnit};
use std::time::{Duration, Instant};

const MAGIC: &[u8; 4] = b"GCHB";
const FORMAT_VERSION: u8 = 3;
const COMPRESSION_NONE_CODE: u8 = 0;
const COMPRESSION_ZSTD_CODE: u8 = 1;
const COMPRESSION_UNIT_BUNDLE_CODE: u8 = 0;
const BUNDLE_TABLE_ENTRY_SIZE: usize = 32 + (8 * 4);

#[derive(Debug, Clone, Default)]
pub struct BundleBuildMetrics {
    pub payload_assembly: Duration,
    pub payload_assembly_bytes: u64,
    pub stream_compression: Duration,
    pub stream_input_bytes: u64,
    pub stream_output_bytes: u64,
    pub serialization: Duration,
    pub serialization_bytes: u64,
}

pub struct BundleBuilder {
    chunks: Vec<ChunkData>,
    pub profile: CompressionProfile,
}

struct ChunkData {
    hash: ChunkHash,
    raw_size: u64,
    raw: Option<ChunkRawData>,
    compressed: Option<Vec<u8>>,
}

enum ChunkRawData {
    Owned(Vec<u8>),
    Shared(Arc<[u8]>),
}

impl ChunkRawData {
    fn as_slice(&self) -> &[u8] {
        match self {
            ChunkRawData::Owned(bytes) => bytes.as_slice(),
            ChunkRawData::Shared(bytes) => bytes.as_ref(),
        }
    }
}

impl BundleBuilder {
    pub fn new(profile: CompressionProfile) -> Self {
        BundleBuilder {
            chunks: Vec::new(),
            profile,
        }
    }

    pub fn add_chunk(&mut self, chunk_hash: ChunkHash, raw_data: Vec<u8>) {
        let raw_size = raw_data.len() as u64;
        self.chunks.push(ChunkData {
            hash: chunk_hash,
            raw_size,
            raw: Some(ChunkRawData::Owned(raw_data)),
            compressed: None,
        });
    }

    pub fn add_shared_chunk(&mut self, chunk_hash: ChunkHash, raw_data: Arc<[u8]>) {
        let raw_size = raw_data.len() as u64;
        self.chunks.push(ChunkData {
            hash: chunk_hash,
            raw_size,
            raw: Some(ChunkRawData::Shared(raw_data)),
            compressed: None,
        });
    }

    pub fn add_precompressed_chunk(
        &mut self,
        chunk_hash: ChunkHash,
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

    pub fn estimated_compressed_size(&self) -> u64 {
        let is_uncompressed = self.profile.compression == CompressionKind::None;
        self.chunks
            .iter()
            .map(|c| {
                if let Some(compressed) = &c.compressed {
                    compressed.len() as u64
                } else if let Some(raw) = &c.raw {
                    if is_uncompressed {
                        raw.as_slice().len() as u64
                    } else {
                        let compressed: std::result::Result<Vec<u8>, _> =
                            compress::compress_with_level(raw.as_slice(), self.profile.zstd_level);
                        compressed
                            .map(|v: Vec<u8>| v.len() as u64)
                            .unwrap_or(u64::MAX)
                    }
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
        let (_, bundle) = self.build_with_id()?;
        Ok(bundle)
    }

    pub fn build_with_id(&self) -> Result<(String, Vec<u8>)> {
        let (bundle_id, bundle, _) = self.build_with_id_and_metrics()?;
        Ok((bundle_id, bundle))
    }

    pub fn build_with_id_and_metrics(&self) -> Result<(String, Vec<u8>, BundleBuildMetrics)> {
        if self.profile.compression_unit != CompressionUnit::Bundle {
            return Err(crate::error::Error::InvalidBundle(
                "gutch only supports bundle-compressed bundles".into(),
            ));
        }
        self.build_bundle_compressed()
    }

    fn build_bundle_compressed(&self) -> Result<(String, Vec<u8>, BundleBuildMetrics)> {
        let is_uncompressed = self.profile.compression == CompressionKind::None;
        let mut entries: Vec<BundleTableEntry> = Vec::with_capacity(self.chunks.len());
        let payload_assembly_start = Instant::now();
        let mut current_raw_offset: u64 = 0;

        for chunk in &self.chunks {
            chunk.raw.as_ref().ok_or_else(|| {
                crate::error::Error::InvalidBundle(
                    "bundle-compressed chunks must retain raw bytes until bundle build".into(),
                )
            })?;

            entries.push(BundleTableEntry {
                chunk_hash: chunk.hash.to_hex(),
                raw_size: chunk.raw_size,
                compressed_size: 0,
                compressed_offset: 0,
                raw_offset: current_raw_offset,
            });
            current_raw_offset += chunk.raw_size;
        }
        let payload_assembly = payload_assembly_start.elapsed();

        let compression_start = Instant::now();
        let compressed_payload = if is_uncompressed {
            let mut payload = Vec::with_capacity(current_raw_offset as usize);
            for chunk in &self.chunks {
                let raw = chunk.raw.as_ref().ok_or_else(|| {
                    crate::error::Error::InvalidBundle(
                        "bundle-compressed chunks must retain raw bytes until bundle build".into(),
                    )
                })?;
                payload.extend_from_slice(raw.as_slice());
            }
            payload
        } else {
            let raw_segments = self
                .chunks
                .iter()
                .map(|chunk| {
                    chunk
                        .raw
                        .as_ref()
                        .map(ChunkRawData::as_slice)
                        .ok_or_else(|| {
                            crate::error::Error::InvalidBundle(
                                "bundle-compressed chunks must retain raw bytes until bundle build"
                                    .into(),
                            )
                        })
                })
                .collect::<Result<Vec<_>>>()?;
            compress::compress_segments_with_level(
                raw_segments,
                current_raw_offset,
                self.profile.zstd_level,
            )?
        };
        let stream_compression = compression_start.elapsed();
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
        header.push(compression_code(self.profile.compression));
        header.push(compression_unit_code(self.profile.compression_unit));
        header.extend_from_slice(&chunk_count.to_le_bytes());
        header.extend_from_slice(&0u32.to_le_bytes());
        let table_len = (entries.len() * BUNDLE_TABLE_ENTRY_SIZE) as u32;
        header.extend_from_slice(&table_len.to_le_bytes());
        write_bundle_table_entries(&mut header, &entries)?;

        let serialization_start = Instant::now();
        let mut bundle = header;
        bundle.extend_from_slice(&compressed_payload);
        let bundle_len = bundle.len() as u64;

        let (bundle_id, bundle) = finalize_bundle_bytes(bundle);
        Ok((
            bundle_id,
            bundle,
            BundleBuildMetrics {
                payload_assembly,
                payload_assembly_bytes: current_raw_offset,
                stream_compression,
                stream_input_bytes: current_raw_offset,
                stream_output_bytes: compressed_payload.len() as u64,
                serialization: serialization_start.elapsed(),
                serialization_bytes: bundle_len,
            },
        ))
    }
}

fn finalize_bundle_bytes(mut bundle_without_id: Vec<u8>) -> (String, Vec<u8>) {
    let bundle_id = hash::hash_hex(&blake3::hash(&bundle_without_id));
    bundle_without_id.extend_from_slice(bundle_id.as_bytes());
    (bundle_id, bundle_without_id)
}

pub struct BundleReader<R: Read + Seek> {
    entries: Vec<BundleTableEntry>,
    chunk_hashes: Vec<ChunkHash>,
    chunk_index_by_hash: HashMap<ChunkHash, usize>,
    data_start: u64,
    payload_size: u64,
    compression: CompressionKind,
    compression_unit: CompressionUnit,
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

        let mut compression_code_buf = [0u8; 1];
        reader.read_exact(&mut compression_code_buf)?;
        let compression = compression_label_from_code(compression_code_buf[0])?;

        let mut compression_unit_code_buf = [0u8; 1];
        reader.read_exact(&mut compression_unit_code_buf)?;
        let compression_unit = compression_unit_from_code(compression_unit_code_buf[0])?;

        let mut count_buf = [0u8; 4];
        reader.read_exact(&mut count_buf)?;
        let chunk_count = u32::from_le_bytes(count_buf) as usize;

        reader.read_exact(&mut count_buf)?;

        reader.read_exact(&mut count_buf)?;
        let table_len = u32::from_le_bytes(count_buf) as usize;
        let (entries, chunk_hashes) =
            read_bundle_table_entries(&mut reader, chunk_count, table_len)?;

        let data_start = reader.stream_position()?;
        let end = reader.seek(SeekFrom::End(0))?;
        let payload_size = end.saturating_sub(data_start).saturating_sub(64);
        reader.seek(SeekFrom::Start(data_start))?;
        let chunk_index_by_hash = chunk_hashes
            .iter()
            .enumerate()
            .map(|(index, chunk_hash)| (*chunk_hash, index))
            .collect();
        Ok(BundleReader {
            entries,
            chunk_hashes,
            chunk_index_by_hash,
            data_start,
            payload_size,
            compression,
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

    pub fn chunk_hashes(&self) -> &[ChunkHash] {
        &self.chunk_hashes
    }

    pub fn chunk_index(&self) -> HashMap<String, usize> {
        self.entries
            .iter()
            .enumerate()
            .map(|(i, e)| (e.chunk_hash.clone(), i))
            .collect()
    }

    pub fn compression_unit(&self) -> CompressionUnit {
        self.compression_unit
    }

    pub fn payload_size(&self) -> u64 {
        self.payload_size
    }

    pub fn stage_bundle_payload_to<W: Write>(&mut self, writer: &mut W) -> Result<u64> {
        self.reader.seek(SeekFrom::Start(self.data_start))?;
        let mut compressed = vec![0u8; self.payload_size as usize];
        self.reader.read_exact(&mut compressed)?;
        let written = compress::decompress_data_to_writer(&compressed, self.compression, writer)?;
        Ok(written)
    }

    pub fn read_chunk_bytes(&mut self, index: usize) -> Result<&[u8]> {
        if index >= self.entries.len() {
            return Err(crate::error::Error::InvalidBundle(format!(
                "chunk index {} out of range ({} chunks)",
                index,
                self.entries.len()
            )));
        }

        self.ensure_bundle_raw_cache()?;

        let entry = &self.entries[index];
        let expected_hash = self.chunk_hashes[index];
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

        let chunk_data = &raw[start..end];
        let actual_hash = ChunkHash::from_blake3(hash::hash_chunk(chunk_data));
        if actual_hash != expected_hash {
            return Err(crate::error::Error::HashMismatch {
                expected: expected_hash.to_hex(),
                actual: actual_hash.to_hex(),
            });
        }

        Ok(chunk_data)
    }

    pub fn read_chunk(&mut self, index: usize) -> Result<Vec<u8>> {
        Ok(self.read_chunk_bytes(index)?.to_vec())
    }

    pub fn read_chunk_by_hash(&mut self, hash: &str) -> Result<Vec<u8>> {
        let expected_hash = ChunkHash::parse_hex(hash)?;
        let index = *self
            .chunk_index_by_hash
            .get(&expected_hash)
            .ok_or_else(|| crate::error::Error::ChunkNotFound(hash.to_string()))?;
        self.read_chunk(index)
    }

    pub fn verify(&mut self) -> Result<bool> {
        for i in 0..self.entries.len() {
            match self.read_chunk_bytes(i) {
                Ok(_) => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(true)
    }

    fn ensure_bundle_raw_cache(&mut self) -> Result<()> {
        if self.bundle_raw_cache.is_some() {
            return Ok(());
        }

        self.reader.seek(SeekFrom::Start(self.data_start))?;
        let mut compressed = vec![0u8; self.payload_size as usize];
        self.reader.read_exact(&mut compressed)?;
        let decompressed = compress::decompress_data(&compressed, self.compression)?;
        self.bundle_raw_cache = Some(decompressed);
        Ok(())
    }
}

fn write_bundle_table_entries(writer: &mut impl Write, entries: &[BundleTableEntry]) -> Result<()> {
    for entry in entries {
        writer.write_all(ChunkHash::parse_hex(&entry.chunk_hash)?.as_bytes())?;
        writer.write_all(&entry.raw_size.to_le_bytes())?;
        writer.write_all(&entry.compressed_size.to_le_bytes())?;
        writer.write_all(&entry.compressed_offset.to_le_bytes())?;
        writer.write_all(&entry.raw_offset.to_le_bytes())?;
    }
    Ok(())
}

fn read_bundle_table_entries(
    reader: &mut impl Read,
    chunk_count: usize,
    table_len: usize,
) -> Result<(Vec<BundleTableEntry>, Vec<ChunkHash>)> {
    let expected_table_len = chunk_count
        .checked_mul(BUNDLE_TABLE_ENTRY_SIZE)
        .ok_or_else(|| crate::error::Error::InvalidBundle("bundle table too large".into()))?;
    if table_len != expected_table_len {
        return Err(crate::error::Error::InvalidBundle(format!(
            "bundle table length {} does not match {} entries",
            table_len, chunk_count
        )));
    }

    let mut entries = Vec::with_capacity(chunk_count);
    let mut chunk_hashes = Vec::with_capacity(chunk_count);
    for _ in 0..chunk_count {
        let chunk_hash = ChunkHash::from_bytes(read_exact_array(reader)?);
        let raw_size = read_u64(reader)?;
        let compressed_size = read_u64(reader)?;
        let compressed_offset = read_u64(reader)?;
        let raw_offset = read_u64(reader)?;
        chunk_hashes.push(chunk_hash);
        entries.push(BundleTableEntry {
            chunk_hash: chunk_hash.to_hex(),
            raw_size,
            compressed_size,
            compressed_offset,
            raw_offset,
        });
    }
    Ok((entries, chunk_hashes))
}

fn read_exact_array<const N: usize>(reader: &mut impl Read) -> Result<[u8; N]> {
    let mut bytes = [0u8; N];
    reader.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn read_u64(reader: &mut impl Read) -> Result<u64> {
    Ok(u64::from_le_bytes(read_exact_array(reader)?))
}

fn compression_code(compression: CompressionKind) -> u8 {
    match compression {
        CompressionKind::None => COMPRESSION_NONE_CODE,
        CompressionKind::Zstd => COMPRESSION_ZSTD_CODE,
    }
}

fn compression_label_from_code(code: u8) -> Result<CompressionKind> {
    match code {
        COMPRESSION_NONE_CODE => Ok(CompressionKind::None),
        COMPRESSION_ZSTD_CODE => Ok(CompressionKind::Zstd),
        other => Err(crate::error::Error::InvalidBundle(format!(
            "unsupported bundle compression code {}",
            other
        ))),
    }
}

fn compression_unit_code(compression_unit: CompressionUnit) -> u8 {
    match compression_unit {
        CompressionUnit::Bundle => COMPRESSION_UNIT_BUNDLE_CODE,
    }
}

fn compression_unit_from_code(code: u8) -> Result<CompressionUnit> {
    match code {
        COMPRESSION_UNIT_BUNDLE_CODE => Ok(CompressionUnit::Bundle),
        other => Err(crate::error::Error::InvalidBundle(format!(
            "unsupported bundle compression unit code {}",
            other
        ))),
    }
}

pub fn read_bundle(data: &[u8]) -> Result<BundleReader<std::io::Cursor<&[u8]>>> {
    let cursor = std::io::Cursor::new(data);
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

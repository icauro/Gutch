use crate::artifact::format;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::manifest::{ArtifactStorageMode, ChunkIndexEntry, CompressionProfile};
use crate::{Error, GutchRepoPaths, Result};

/// A persisted artifact that can satisfy one or more logical chunks.
///
/// Bundles and loose chunks differ only at this boundary. Snapshot planning,
/// reconstruction, and verification operate on this type; the remote transfer
/// layer resolves bundle URLs at its provider-specific boundary.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum ArtifactRef {
    Bundle { id: String },
    LooseChunk { hash: String },
}

impl ArtifactRef {
    pub(crate) fn from_chunk_entry(chunk_hash: &str, entry: &ChunkIndexEntry) -> Self {
        match entry.storage_mode {
            ArtifactStorageMode::Bundled => Self::Bundle {
                id: entry.bundle_id.to_string(),
            },
            ArtifactStorageMode::LooseChunks => Self::LooseChunk {
                hash: chunk_hash.to_string(),
            },
        }
    }

    pub(crate) fn id(&self) -> &str {
        match self {
            Self::Bundle { id } => id,
            Self::LooseChunk { hash } => hash,
        }
    }

    pub(crate) fn local_path(&self, repo_paths: &GutchRepoPaths) -> Result<PathBuf> {
        match self {
            Self::Bundle { id } => crate::bundle_path(repo_paths, id),
            Self::LooseChunk { hash } => crate::chunk_path(repo_paths, hash),
        }
    }
}

pub(crate) fn required_artifacts(
    snapshot_manifest: &crate::manifest::snapshot::SnapshotManifest,
    chunk_index: &crate::manifest::ChunkIndex,
) -> Result<std::collections::HashSet<ArtifactRef>> {
    snapshot_manifest
        .files()
        .flat_map(|file| file.chunks.iter())
        .map(|chunk_ref| {
            let entry = chunk_index
                .entries
                .get(&chunk_ref.chunk_hash)
                .ok_or_else(|| {
                    Error::manifest(format!(
                        "missing chunk index entry for required chunk '{}'",
                        chunk_ref.chunk_hash
                    ))
                })?;
            if entry.raw_size != chunk_ref.raw_size {
                return Err(Error::manifest(format!(
                    "chunk index size disagrees with snapshot for '{}'",
                    chunk_ref.chunk_hash
                )));
            }
            Ok(ArtifactRef::from_chunk_entry(&chunk_ref.chunk_hash, entry))
        })
        .collect()
}

pub(crate) fn read_loose_chunk(
    path: &std::path::Path,
    chunk_hash: &str,
    raw_size: u64,
    compression_profile: &CompressionProfile,
) -> Result<Vec<u8>> {
    read_loose_chunk_with_profiler(path, chunk_hash, raw_size, compression_profile, None)
}

pub(crate) fn read_loose_chunk_with_profiler(
    path: &std::path::Path,
    chunk_hash: &str,
    raw_size: u64,
    compression_profile: &CompressionProfile,
    profiler: Option<&crate::GutchProfiler>,
) -> Result<Vec<u8>> {
    Ok(read_loose_chunk_with_metrics(path, chunk_hash, raw_size, compression_profile, profiler)?.0)
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct LooseChunkReadMetrics {
    pub(crate) compressed_bytes: u64,
    pub(crate) raw_bytes: u64,
    pub(crate) read: Duration,
    pub(crate) decompress: Duration,
    pub(crate) size_verify: Duration,
    pub(crate) hash_verify: Duration,
}

pub(crate) fn read_loose_chunk_with_metrics(
    path: &std::path::Path,
    chunk_hash: &str,
    raw_size: u64,
    _compression_profile: &CompressionProfile,
    profiler: Option<&crate::GutchProfiler>,
) -> Result<(Vec<u8>, LooseChunkReadMetrics)> {
    let mut metrics = LooseChunkReadMetrics::default();
    let (mut file, codec) = {
        let started_at = Instant::now();
        let _read_span =
            profiler.map(|profiler| profiler.span("artifact.loose_chunk.payload_read"));
        let mut file = std::fs::File::open(path)?;
        metrics.compressed_bytes = file.metadata()?.len();
        format::size_u32(metrics.compressed_bytes)?;
        let (declared_hash, declared_size, codec) = read_loose_header(&mut file)?;
        if declared_hash.to_hex() != chunk_hash || u64::from(declared_size) != raw_size {
            return Err(Error::manifest(
                "loose chunk header disagrees with requested content",
            ));
        }
        metrics.read = started_at.elapsed();
        (file, codec)
    };
    let bytes = {
        let started_at = Instant::now();
        let _decompress_span =
            profiler.map(|profiler| profiler.span("artifact.loose_chunk.payload_decompress"));
        let mut decoder =
            crate::compress::decoder_from_reader(&mut file, codec)?.take(raw_size + 1);
        let mut bytes = Vec::with_capacity(raw_size as usize);
        decoder.read_to_end(&mut bytes)?;
        metrics.decompress = started_at.elapsed();
        metrics.raw_bytes = bytes.len() as u64;
        bytes
    };
    {
        let started_at = Instant::now();
        let _size_span = profiler.map(|profiler| profiler.span("artifact.loose_chunk.size_verify"));
        if bytes.len() as u64 != raw_size {
            return Err(Error::reconstruct(format!(
                "loose chunk '{}' has size {}, expected {}",
                chunk_hash,
                bytes.len(),
                raw_size
            )));
        }
        metrics.size_verify = started_at.elapsed();
    }
    {
        let started_at = Instant::now();
        let _hash_span = profiler.map(|profiler| profiler.span("artifact.loose_chunk.hash_verify"));
        let actual_hash = crate::hash::hash_hex(&crate::hash::hash_chunk(&bytes));
        if actual_hash != chunk_hash {
            return Err(Error::HashMismatch {
                expected: chunk_hash.to_string(),
                actual: actual_hash,
            });
        }
        metrics.hash_verify = started_at.elapsed();
    }
    Ok((bytes, metrics))
}

pub(crate) fn write_loose_chunk(
    writer: &mut impl Write,
    hash: crate::ChunkHash,
    bytes: &[u8],
    profile: &CompressionProfile,
) -> Result<()> {
    if bytes.is_empty() || bytes.len() as u64 > format::MAX_RAW_CHUNK_SIZE {
        return Err(Error::manifest("loose chunk exceeds raw size limit"));
    }
    writer.write_all(format::LOOSE_CHUNK_MAGIC)?;
    writer.write_all(&format::VERSION.to_le_bytes())?;
    format::write_codec(writer, profile.compression)?;
    writer.write_all(&format::size_u32(bytes.len() as u64)?.to_le_bytes())?;
    writer.write_all(hash.as_bytes())?;
    crate::compress::encode_to_writer(writer, profile, bytes.len() as u64, |writer| {
        writer.write_all(bytes)?;
        Ok(())
    })
}

pub(crate) fn read_loose_header(
    reader: &mut impl Read,
) -> Result<(crate::ChunkHash, u32, crate::manifest::CompressionKind)> {
    let mut magic = [0u8; 16];
    reader.read_exact(&mut magic)?;
    if &magic != format::LOOSE_CHUNK_MAGIC {
        return Err(Error::manifest("invalid loose chunk magic"));
    }
    format::read_version(reader)?;
    let codec = format::read_codec(reader)?;
    let mut size = [0u8; 4];
    reader.read_exact(&mut size)?;
    let size = u32::from_le_bytes(size);
    if size == 0 || u64::from(size) > format::MAX_RAW_CHUNK_SIZE {
        return Err(Error::manifest("invalid loose chunk raw size"));
    }
    let mut hash = [0u8; 32];
    reader.read_exact(&mut hash)?;
    Ok((crate::ChunkHash::from_bytes(hash), size, codec))
}

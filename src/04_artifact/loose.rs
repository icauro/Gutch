//! Loose-chunk codec: one compressed chunk per artifact file.

use crate::artifact::format;
use crate::profiling::ProfileInstant;
use std::io::Read;
use std::io::Write;
use std::time::Duration;

use crate::Error;
use crate::Result;
use crate::artifact::compression::decoder_from_reader;
use crate::artifact::compression::encode_to_writer;
use crate::artifact::hash::ChunkHash;
use crate::artifact::profiles::CompressionKind;
use crate::artifact::profiles::CompressionProfile;
use crate::profiling::Profiler;
use std::fs::File;
use std::path::Path;

pub(crate) fn read_loose_chunk(
    path: &Path,
    chunk_hash: &str,
    raw_size: u64,
    compression_profile: &CompressionProfile,
) -> Result<Vec<u8>> {
    read_loose_chunk_with_profiler(path, chunk_hash, raw_size, compression_profile, None)
}

pub(crate) fn read_loose_chunk_with_profiler(
    path: &Path,
    chunk_hash: &str,
    raw_size: u64,
    compression_profile: &CompressionProfile,
    profiler: Option<&Profiler>,
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
    path: &Path,
    chunk_hash: &str,
    raw_size: u64,
    _compression_profile: &CompressionProfile,
    profiler: Option<&Profiler>,
) -> Result<(Vec<u8>, LooseChunkReadMetrics)> {
    let mut metrics = LooseChunkReadMetrics::default();
    let (mut file, codec) = {
        let started_at = ProfileInstant::detailed(profiler);

        let mut file = File::open(path)?;
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
        let started_at = ProfileInstant::detailed(profiler);

        let mut decoder = decoder_from_reader(&mut file, codec)?.take(raw_size + 1);
        let mut bytes = Vec::with_capacity(raw_size as usize);
        decoder.read_to_end(&mut bytes)?;
        metrics.decompress = started_at.elapsed();
        metrics.raw_bytes = bytes.len() as u64;
        bytes
    };
    {
        let started_at = ProfileInstant::detailed(profiler);

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
        let started_at = ProfileInstant::detailed(profiler);

        let actual_hash = ChunkHash::of(&bytes).to_hex();
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
    hash: ChunkHash,
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
    encode_to_writer(writer, profile, bytes.len() as u64, |writer| {
        writer.write_all(bytes)?;
        Ok(())
    })
}

pub(crate) fn read_loose_header(
    reader: &mut impl Read,
) -> Result<(ChunkHash, u32, CompressionKind)> {
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
    Ok((ChunkHash::from_bytes(hash), size, codec))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maximum_chunk_roundtrips_with_each_embedded_codec() {
        let directory = tempfile::tempdir().unwrap();
        let bytes = vec![97; format::MAX_RAW_CHUNK_SIZE as usize];
        let hash = crate::artifact::hash::ChunkHash::from_bytes(*blake3::hash(&bytes).as_bytes());
        for codec in [
            crate::artifact::profiles::CompressionKind::None,
            crate::artifact::profiles::CompressionKind::Zstd,
            crate::artifact::profiles::CompressionKind::Lz4,
        ] {
            let path = directory.path().join(format!("{codec:?}"));
            let profile = CompressionProfile::for_algorithm(codec, None);
            write_loose_chunk(
                &mut std::fs::File::create(&path).unwrap(),
                hash,
                &bytes,
                &profile,
            )
            .unwrap();
            // The stored codec remains authoritative after repository defaults change.
            let decoded = read_loose_chunk(
                &path,
                &hash.to_hex(),
                bytes.len() as u64,
                &CompressionProfile::default(),
            )
            .unwrap();
            assert_eq!(decoded, bytes);
            let mut encoded = std::fs::read(&path).unwrap();
            encoded[20..24]
                .copy_from_slice(&((format::MAX_RAW_CHUNK_SIZE + 1) as u32).to_le_bytes());
            assert!(read_loose_header(&mut &encoded[..]).is_err());
        }
    }
}

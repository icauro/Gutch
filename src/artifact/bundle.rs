use super::binary_io::{read_exact_array, read_u32};
use crate::profile::ProfileInstant;
#[cfg(test)]
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::io::Seek;
use std::io::SeekFrom;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use crate::artifact::compression::validate_profile;
use crate::artifact::hash;
use crate::artifact::hash::ChunkHash;
use crate::artifact::profiles::CompressionKind;
use crate::artifact::profiles::CompressionProfile;
use crate::artifact::profiles::CompressionUnit;
use crate::error::Result;

use crate::artifact::format;
use crate::artifact::format::BUNDLE_FOOTER_MAGIC as FOOTER_MAGIC;
use crate::artifact::format::BUNDLE_MAGIC;
const BUNDLE_HEADER_SIZE: u64 = 16 + 2 + 2 + 4;
const BUNDLE_TRAILER_SIZE: u64 = 4 + 16;
const BUNDLE_ID_SIZE: u64 = 32;
const BUNDLE_TABLE_ENTRY_SIZE: usize = 36;

pub(crate) fn encoded_metadata_size(chunk_count: u64) -> u64 {
    BUNDLE_HEADER_SIZE
        + 4
        + chunk_count * BUNDLE_TABLE_ENTRY_SIZE as u64
        + BUNDLE_TRAILER_SIZE
        + BUNDLE_ID_SIZE
}

const STREAM_BUFFER_SIZE: usize = 1024 * 1024;

#[derive(Debug, Clone, Default)]
pub(crate) struct BundleBuildMetrics {
    pub(crate) frame_compression: Duration,
    pub(crate) frame_input_bytes: u64,
    pub(crate) frame_output_bytes: u64,
    pub(crate) footer_serialization: Duration,
    pub(crate) footer_serialization_bytes: u64,
}

#[derive(Debug)]
pub(crate) struct BundleBuildArtifact {
    pub(crate) bundle_id: String,
    pub(crate) staged_path: PathBuf,
    pub(crate) size: u64,
    pub(crate) entries: Vec<BundleTableEntry>,
    pub(crate) metrics: BundleBuildMetrics,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct BundleReadMetrics {
    pub(crate) raw_bytes: u64,
    pub(crate) compressed_bytes: u64,
    pub(crate) read: Duration,
    pub(crate) decompress: Duration,
}

#[cfg(test)]
pub(crate) struct BundleBuilder {
    chunks: Vec<ChunkData>,
    pub(crate) profile: CompressionProfile,
}

#[cfg(test)]
struct ChunkData {
    hash: ChunkHash,
    raw_size: u64,
    raw: Vec<u8>,
}

#[cfg(test)]
impl BundleBuilder {
    pub(crate) fn new(profile: CompressionProfile) -> Self {
        Self {
            chunks: Vec::new(),
            profile,
        }
    }

    pub(crate) fn add_chunk(&mut self, chunk_hash: ChunkHash, raw_data: Vec<u8>) {
        let raw_size = raw_data.len() as u64;
        self.chunks.push(ChunkData {
            hash: chunk_hash,
            raw_size,
            raw: raw_data,
        });
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }

    pub(crate) fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    /// Test-only convenience. Production capture writes to a staged file.
    pub(crate) fn build(&self) -> Result<Vec<u8>> {
        let (_, bundle) = self.build_with_id()?;
        Ok(bundle)
    }

    /// Test-only convenience. Production capture writes to a staged file.
    pub(crate) fn build_with_id(&self) -> Result<(String, Vec<u8>)> {
        let staging = tempfile::tempdir()?;
        let artifact = self.write_to_staging(staging.path(), None)?;
        let bytes = std::fs::read(&artifact.staged_path)?;
        let bundle_id = artifact.bundle_id.clone();
        drop(artifact);
        Ok((bundle_id, bytes))
    }

    pub(crate) fn write_to_staging(
        &self,
        staging_root: &Path,
        profiler: Option<&crate::profile::GutchProfiler>,
    ) -> Result<BundleBuildArtifact> {
        let chunks = self
            .chunks
            .iter()
            .map(|chunk| (chunk.hash, chunk.raw_size))
            .collect::<Vec<_>>();
        write_bundle_to_staging(&chunks, &self.profile, staging_root, profiler, |index| {
            Ok(self.chunks[index].raw.as_slice())
        })
    }
}

/// Encode ordered chunks without retaining the complete raw payload.
/// The loader is called once per descriptor; each chunk is released before loading the next.
pub(crate) fn write_bundle_to_staging<B: AsRef<[u8]>>(
    chunks: &[(ChunkHash, u64)],
    profile: &CompressionProfile,
    staging_root: &Path,
    profiler: Option<&crate::profile::GutchProfiler>,
    mut load_chunk: impl FnMut(usize) -> Result<B>,
) -> Result<BundleBuildArtifact> {
    if profile.compression_unit != CompressionUnit::Bundle {
        return Err(crate::error::Error::InvalidBundle(
            "gutch only supports bundle-compressed bundles".into(),
        ));
    }
    crate::filesystem::create_dir_all(staging_root)?;

    let temporary = tempfile::NamedTempFile::new_in(staging_root)?;
    let capture = {
        let mut file = temporary.reopen()?;
        let capture = write_bundle(chunks, profile, &mut file, profiler, &mut load_chunk)?;
        let _flush_span = profiler.map(|profiler| {
            profiler.span_with_args("capture.bundle.flush", || {
                std::collections::BTreeMap::from([(
                    "bundle_size_bytes".to_string(),
                    serde_json::json!(capture.size),
                )])
            })
        });
        file.flush()?;
        drop(_flush_span);
        let _sync_span = profiler.map(|profiler| {
            profiler.span_with_args("capture.bundle.sync", || {
                std::collections::BTreeMap::from([(
                    "bundle_size_bytes".to_string(),
                    serde_json::json!(capture.size),
                )])
            })
        });
        file.sync_all()?;
        drop(_sync_span);
        capture
    };

    let (_, staged_path) = temporary
        .keep()
        .map_err(|error| crate::error::Error::Io(error.error))?;

    Ok(BundleBuildArtifact {
        staged_path,
        ..capture
    })
}

fn write_bundle<B: AsRef<[u8]>>(
    chunks: &[(ChunkHash, u64)],
    profile: &CompressionProfile,
    file: &mut File,
    profiler: Option<&crate::profile::GutchProfiler>,
    load_chunk: &mut impl FnMut(usize) -> Result<B>,
) -> Result<BundleBuildArtifact> {
    let chunk_count = u32::try_from(chunks.len()).map_err(|_| {
        crate::error::Error::InvalidBundle("bundle contains too many chunks".into())
    })?;
    let mut hashing = HashingWriter::new(file);
    hashing.write_all(BUNDLE_MAGIC)?;
    hashing.write_all(&format::VERSION.to_le_bytes())?;
    format::write_codec(&mut hashing, profile.compression)?;
    hashing.write_all(&chunk_count.to_le_bytes())?;

    let mut entries = Vec::with_capacity(chunks.len());
    let mut raw_offset = 0u64;
    for &(chunk_hash, raw_size) in chunks {
        entries.push(BundleTableEntry {
            chunk_hash,
            raw_size: format::size_u32(raw_size)?,
            compressed_size: 0,
            raw_offset: format::size_u32(raw_offset)?,
        });
        raw_offset = raw_offset
            .checked_add(raw_size)
            .ok_or_else(|| crate::error::Error::InvalidBundle("bundle raw size overflow".into()))?;
    }

    validate_bundle_entries(&entries)?;
    let compression_started_at = ProfileInstant::detailed(profiler);
    let _payload_span = profiler.map(|profiler| {
        profiler.span_with_args("capture.bundle.encode_and_write", || {
            std::collections::BTreeMap::from([(
                "chunk_count".to_string(),
                serde_json::json!(chunks.len()),
            )])
        })
    });
    let raw_size = raw_offset;
    validate_profile(profile)?;
    if raw_size > format::MAX_RAW_BUNDLE_SIZE {
        return Err(crate::Error::bundle("bundle exceeds raw size limit"));
    }
    let mut input_load_time = Duration::ZERO;
    crate::artifact::compression::encode_to_writer(&mut hashing, profile, raw_size, |writer| {
        for (index, &(_, expected_size)) in chunks.iter().enumerate() {
            let load_started = ProfileInstant::detailed(profiler);
            let bytes = load_chunk(index)?;
            input_load_time += load_started.elapsed();
            let bytes = bytes.as_ref();
            if bytes.len() as u64 != expected_size {
                return Err(crate::Error::bundle(
                    "bundle chunk raw size changed before writing",
                ));
            }
            writer.write_all(bytes)?;
        }
        Ok(())
    })?;
    let compression_time = compression_started_at
        .elapsed()
        .saturating_sub(input_load_time);
    let payload_end = hashing.position()?;
    let compressed_output_bytes = payload_end.checked_sub(BUNDLE_HEADER_SIZE).ok_or_else(|| {
        crate::error::Error::InvalidBundle("bundle payload position underflow".into())
    })?;
    assign_compressed_size_estimates(&mut entries, compressed_output_bytes)?;
    drop(_payload_span);
    let serialization_started_at = ProfileInstant::detailed(profiler);

    let mut footer = Vec::with_capacity(4 + entries.len() * BUNDLE_TABLE_ENTRY_SIZE);
    footer.extend_from_slice(&chunk_count.to_le_bytes());
    write_bundle_table_entries(&mut footer, &entries)?;
    let footer_length = u64::try_from(footer.len())
        .map_err(|_| crate::error::Error::InvalidBundle("bundle footer is too large".into()))?;
    hashing.write_all(&footer)?;
    hashing.write_all(&format::size_u32(footer_length)?.to_le_bytes())?;
    hashing.write_all(FOOTER_MAGIC)?;
    let bytes_without_id = hashing.position()?;

    let HashingWriter { file, hasher } = hashing;
    let bundle_id = hash::hash_hex(&hasher.finalize());
    file.write_all(hasher.finalize().as_bytes())?;
    let bundle_size = bytes_without_id
        .checked_add(BUNDLE_ID_SIZE)
        .ok_or_else(|| crate::error::Error::InvalidBundle("bundle size overflow".into()))?;
    format::size_u32(bundle_size)?;

    Ok(BundleBuildArtifact {
        bundle_id,
        staged_path: PathBuf::new(),
        size: bundle_size,
        entries,
        metrics: BundleBuildMetrics {
            frame_compression: compression_time,
            frame_input_bytes: raw_offset,
            frame_output_bytes: compressed_output_bytes,
            footer_serialization: serialization_started_at.elapsed(),
            footer_serialization_bytes: bytes_without_id,
        },
    })
}
struct HashingWriter<'a> {
    file: &'a mut File,
    hasher: blake3::Hasher,
}

impl<'a> HashingWriter<'a> {
    fn new(file: &'a mut File) -> Self {
        Self {
            file,
            hasher: blake3::Hasher::new(),
        }
    }

    fn position(&mut self) -> Result<u64> {
        Ok(self.file.stream_position()?)
    }
}

impl Write for HashingWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let written = self.file.write(bytes)?;
        self.hasher.update(&bytes[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

pub(crate) struct BundleReader<R: Read + Seek> {
    entries: Vec<BundleTableEntry>,
    #[cfg(test)]
    chunk_index_by_hash: HashMap<ChunkHash, usize>,
    data_start: u64,
    payload_end: u64,
    compression: CompressionKind,
    #[cfg(test)]
    compression_unit: CompressionUnit,
    reader: R,
}

impl<R: Read + Seek> BundleReader<R> {
    pub(crate) fn open(mut reader: R, _compression_profile: &CompressionProfile) -> Result<Self> {
        let end = reader.seek(SeekFrom::End(0))?;
        let minimum_size = BUNDLE_HEADER_SIZE + BUNDLE_TRAILER_SIZE + BUNDLE_ID_SIZE;
        format::size_u32(end)?;
        if end < minimum_size {
            return Err(crate::error::Error::InvalidBundle(
                "bundle is smaller than its fixed framing".into(),
            ));
        }
        reader.seek(SeekFrom::Start(0))?;
        if &read_exact_array::<16>(&mut reader)? != BUNDLE_MAGIC {
            return Err(crate::error::Error::InvalidBundle(
                "bundle magic is invalid".into(),
            ));
        }
        format::read_version(&mut reader)?;
        let compression = format::read_codec(&mut reader)?;
        let chunk_count = read_u32(&mut reader)? as usize;
        let data_start = BUNDLE_HEADER_SIZE;

        reader.seek(SeekFrom::Start(end - BUNDLE_ID_SIZE - BUNDLE_TRAILER_SIZE))?;
        let footer_length = u64::from(read_u32(&mut reader)?);
        if &read_exact_array::<16>(&mut reader)? != FOOTER_MAGIC {
            return Err(crate::error::Error::InvalidBundle(
                "bundle footer magic is invalid".into(),
            ));
        }
        let footer_start = end
            .checked_sub(BUNDLE_ID_SIZE + BUNDLE_TRAILER_SIZE)
            .and_then(|position| position.checked_sub(footer_length))
            .ok_or_else(|| {
                crate::error::Error::InvalidBundle("bundle footer position underflow".into())
            })?;
        if footer_start < data_start {
            return Err(crate::error::Error::InvalidBundle(
                "bundle footer overlaps its header".into(),
            ));
        }
        let footer_length_usize = usize::try_from(footer_length).map_err(|_| {
            crate::error::Error::InvalidBundle(
                "bundle footer is too large for this platform".into(),
            )
        })?;
        reader.seek(SeekFrom::Start(footer_start))?;
        if footer_length_usize
            != 4usize
                .checked_add(
                    chunk_count
                        .checked_mul(BUNDLE_TABLE_ENTRY_SIZE)
                        .ok_or_else(|| crate::Error::bundle("bundle table size overflow"))?,
                )
                .ok_or_else(|| crate::Error::bundle("bundle footer size overflow"))?
        {
            return Err(crate::Error::bundle(
                "bundle footer length disagrees with chunk count",
            ));
        }
        let mut footer = vec![0u8; footer_length_usize];
        reader.read_exact(&mut footer)?;
        let mut footer_reader = std::io::Cursor::new(footer.as_slice());
        let footer_chunk_count = read_u32(&mut footer_reader)? as usize;
        if footer_chunk_count != chunk_count {
            return Err(crate::error::Error::InvalidBundle(format!(
                "bundle header declares {} chunks but footer declares {}",
                chunk_count, footer_chunk_count
            )));
        }
        let table_length = footer_length_usize.checked_sub(4).ok_or_else(|| {
            crate::error::Error::InvalidBundle("bundle footer is missing its table".into())
        })?;
        let mut entries = read_bundle_table_entries(&mut footer_reader, chunk_count, table_length)?;
        if footer_reader.position() != footer_length {
            return Err(crate::error::Error::InvalidBundle(
                "bundle footer contains trailing bytes".into(),
            ));
        }
        assign_compressed_size_estimates(&mut entries, footer_start - data_start)?;
        validate_bundle_entries(&entries)?;

        #[cfg(test)]
        let chunk_index_by_hash = entries
            .iter()
            .enumerate()
            .map(|(index, entry)| (entry.chunk_hash, index))
            .collect();
        reader.seek(SeekFrom::Start(data_start))?;
        Ok(Self {
            entries,
            #[cfg(test)]
            chunk_index_by_hash,
            data_start,
            payload_end: footer_start,
            compression,
            #[cfg(test)]
            compression_unit: CompressionUnit::Bundle,
            reader,
        })
    }

    #[cfg(test)]
    pub(crate) fn chunk_count(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn entries(&self) -> &[BundleTableEntry] {
        &self.entries
    }

    #[cfg(test)]
    pub(crate) fn compression_unit(&self) -> CompressionUnit {
        self.compression_unit
    }

    /// Read and decompress the complete bundle payload.
    ///
    /// This compatibility helper uses the streaming decoder internally, but
    /// retains the complete decoded payload for callers such as verification
    /// that need random access to every chunk.
    #[cfg(test)]
    pub(crate) fn read_bundle_payload(&mut self) -> Result<(Vec<u8>, BundleReadMetrics)> {
        self.read_bundle_payload_with_profiler(None)
    }

    #[cfg(test)]
    pub(crate) fn read_bundle_payload_with_profiler(
        &mut self,
        profiler: Option<&crate::profile::GutchProfiler>,
    ) -> Result<(Vec<u8>, BundleReadMetrics)> {
        let required = vec![true; self.entries.len()];
        let expected_raw_size = self.entries.iter().try_fold(0u64, |total, entry| {
            total.checked_add(u64::from(entry.raw_size)).ok_or_else(|| {
                crate::error::Error::InvalidBundle("bundle raw size overflow".into())
            })
        })?;
        let decoded_capacity = usize::try_from(expected_raw_size).map_err(|_| {
            crate::error::Error::InvalidBundle(
                "bundle payload is too large for this platform".into(),
            )
        })?;
        let mut bytes = Vec::with_capacity(decoded_capacity);
        let metrics =
            self.stream_bundle_chunks_with_profiler(profiler, &required, |_, chunk| {
                bytes.extend_from_slice(chunk);
                Ok(())
            })?;
        if bytes.len() as u64 != metrics.raw_bytes {
            return Err(crate::error::Error::InvalidBundle(format!(
                "bundle payload has raw size {}, expected {}",
                bytes.len(),
                metrics.raw_bytes
            )));
        }
        Ok((bytes, metrics))
    }

    /// Stream a bundle's decoded chunks in raw-offset order.
    ///
    /// The compressed payload is read once, then decoded incrementally. Only
    /// chunks selected by `required` are materialized and passed to the
    /// callback; unselected chunks are decoded into a reusable discard buffer.
    /// This lets reconstruction verify and fan out a chunk before decoding the
    /// next one without changing the one-frame-per-bundle format.
    pub(crate) fn stream_bundle_chunks_with_profiler<F>(
        &mut self,
        profiler: Option<&crate::profile::GutchProfiler>,
        required: &[bool],
        mut on_chunk: F,
    ) -> Result<BundleReadMetrics>
    where
        F: FnMut(usize, &[u8]) -> Result<()>,
    {
        if required.len() != self.entries.len() {
            return Err(crate::error::Error::InvalidBundle(format!(
                "bundle stream selection has {} entries, expected {}",
                required.len(),
                self.entries.len()
            )));
        }

        let compressed_size = self
            .payload_end
            .checked_sub(self.data_start)
            .ok_or_else(|| {
                crate::error::Error::InvalidBundle("bundle payload position underflow".into())
            })?;
        let expected_raw_size = self.entries.iter().try_fold(0u64, |total, entry| {
            total.checked_add(u64::from(entry.raw_size)).ok_or_else(|| {
                crate::error::Error::InvalidBundle("bundle raw size overflow".into())
            })
        })?;

        let read_started_at = ProfileInstant::detailed(profiler);

        self.reader.seek(SeekFrom::Start(0))?;
        let mut hasher = blake3::Hasher::new();
        let mut header = [0u8; BUNDLE_HEADER_SIZE as usize];
        self.reader.read_exact(&mut header)?;
        hasher.update(&header);
        let read = read_started_at.elapsed();

        let mut decompress = Duration::default();
        let mut encoded = HashingReader {
            reader: (&mut self.reader).take(compressed_size),
            hasher: &mut hasher,
        };
        let mut decoder =
            crate::artifact::compression::decoder_from_reader(&mut encoded, self.compression)?;
        let stream_result = stream_decoded_chunks(
            &mut decoder,
            &self.entries,
            required,
            &mut decompress,
            profiler,
            &mut on_chunk,
        );
        stream_result?;
        drop(decoder);
        std::io::copy(&mut encoded, &mut std::io::sink())?;
        if encoded.reader.limit() != 0 {
            return Err(crate::Error::bundle("truncated bundle payload"));
        }
        let end = self.reader.seek(SeekFrom::End(0))?;
        self.reader.seek(SeekFrom::Start(self.payload_end))?;
        let metadata_length = end - BUNDLE_ID_SIZE - self.payload_end;
        std::io::copy(
            &mut HashingReader {
                reader: (&mut self.reader).take(metadata_length),
                hasher: &mut hasher,
            },
            &mut std::io::sink(),
        )?;
        let trailer = read_exact_array::<32>(&mut self.reader)?;
        if trailer != *hasher.finalize().as_bytes() {
            return Err(crate::Error::bundle("bundle content identity mismatch"));
        }
        // Keep the timing metric focused on decoder reads. The callback can
        // perform hashing and positional output writes, which are accounted
        // for separately by reconstruction timings.

        Ok(BundleReadMetrics {
            raw_bytes: expected_raw_size,
            compressed_bytes: compressed_size,
            read,
            decompress,
        })
    }

    #[cfg(test)]
    pub(crate) fn read_chunk(&mut self, index: usize) -> Result<Vec<u8>> {
        let entry = self.entries.get(index).ok_or_else(|| {
            crate::error::Error::InvalidBundle(format!(
                "chunk index {} out of range ({} chunks)",
                index,
                self.entries.len()
            ))
        })?;
        let raw_offset = usize::try_from(entry.raw_offset).map_err(|_| {
            crate::error::Error::InvalidBundle("bundle raw offset is too large".into())
        })?;
        let raw_size = usize::try_from(entry.raw_size).map_err(|_| {
            crate::error::Error::InvalidBundle("bundle raw size is too large".into())
        })?;
        let raw_end = raw_offset
            .checked_add(raw_size)
            .ok_or_else(|| crate::error::Error::InvalidBundle("bundle raw size overflow".into()))?;
        let (bytes, _) = self.read_bundle_payload()?;
        Ok(bytes[raw_offset..raw_end].to_vec())
    }

    #[cfg(test)]
    pub(crate) fn read_chunk_by_hash(&mut self, hash: &str) -> Result<Vec<u8>> {
        let expected_hash = ChunkHash::parse_hex(hash)?;
        let index = *self
            .chunk_index_by_hash
            .get(&expected_hash)
            .ok_or_else(|| crate::error::Error::ChunkNotFound(hash.to_string()))?;
        self.read_chunk(index)
    }

    pub(crate) fn verify_identity(&mut self, expected_id: &str) -> Result<()> {
        let expected = ChunkHash::parse_hex(expected_id)?;
        self.reader.seek(SeekFrom::End(-(BUNDLE_ID_SIZE as i64)))?;
        if read_exact_array::<32>(&mut self.reader)?.as_slice() != expected.as_bytes() {
            return Err(crate::Error::bundle(
                "bundle filename and trailing identity disagree",
            ));
        }
        self.verify()?;
        Ok(())
    }

    pub(crate) fn verify(&mut self) -> Result<bool> {
        let hashes: Vec<_> = self.entries.iter().map(|entry| entry.chunk_hash).collect();
        let required = vec![true; hashes.len()];
        self.stream_bundle_chunks_with_profiler(None, &required, |index, bytes| {
            let actual = ChunkHash::of(bytes);
            if actual != hashes[index] {
                return Err(crate::Error::HashMismatch {
                    expected: hashes[index].to_hex(),
                    actual: actual.to_hex(),
                });
            }
            Ok(())
        })?;
        Ok(true)
    }
}

/// Verifies complete bundle identity for in-memory bytes: the BLAKE3 of all
/// bytes before the trailing 32-byte binary ID must equal the expected ID, and
/// the trailing bytes must repeat it. Shared by local verification and
/// remote download so both reject the same identity failures.
#[cfg(test)]
pub(crate) fn verify_bundle_identity_bytes(bundle_id: &str, bytes: &[u8]) -> Result<()> {
    let expected_hash = ChunkHash::parse_hex(bundle_id)?;
    let id_size = BUNDLE_ID_SIZE as usize;
    if bytes.len() < id_size {
        return Err(crate::error::Error::InvalidBundle(format!(
            "bundle '{bundle_id}' is missing its content hash"
        )));
    }
    let (content, trailing) = bytes.split_at(bytes.len() - id_size);
    check_bundle_identity(bundle_id, expected_hash, ChunkHash::of(content), trailing)
}

/// Verifies complete bundle identity for a stored bundle file without
/// loading it: the path-derived ID, the trailing ID, and the BLAKE3 of the
/// pre-ID bytes must all match the expected manifest ID.
pub(crate) fn verify_bundle_identity_reader<R: Read + Seek>(
    file: &mut R,
    bundle_id: &str,
) -> Result<()> {
    let expected_hash = ChunkHash::parse_hex(bundle_id)?;
    let id_size = BUNDLE_ID_SIZE as usize;
    let total_len = file.seek(SeekFrom::End(0))?;
    file.seek(SeekFrom::Start(0))?;
    let content_len = total_len.checked_sub(id_size as u64).ok_or_else(|| {
        crate::error::Error::InvalidBundle(format!(
            "bundle '{bundle_id}' is missing its content hash"
        ))
    })?;
    let mut hasher = blake3::Hasher::new();
    let mut remaining = content_len;
    crate::artifact::integrity::with_hash_read_buffer(|buffer| {
        while remaining > 0 {
            let take = remaining.min(buffer.len() as u64) as usize;
            file.read_exact(&mut buffer[..take])?;
            hasher.update(&buffer[..take]);
            remaining -= take as u64;
        }
        Ok(())
    })?;
    let mut trailing = [0u8; BUNDLE_ID_SIZE as usize];
    file.read_exact(&mut trailing)?;
    check_bundle_identity(
        bundle_id,
        expected_hash,
        ChunkHash::from_blake3(hasher.finalize()),
        &trailing,
    )
}

/// The single bundle identity rule: the BLAKE3 of the pre-ID content must equal
/// the expected ID, and the trailing 32 bytes must repeat it.
fn check_bundle_identity(
    bundle_id: &str,
    expected_hash: ChunkHash,
    content_hash: ChunkHash,
    trailing: &[u8],
) -> Result<()> {
    if content_hash != expected_hash {
        return Err(crate::error::Error::HashMismatch {
            expected: expected_hash.to_hex(),
            actual: content_hash.to_hex(),
        });
    }
    if trailing != expected_hash.as_bytes() {
        return Err(crate::error::Error::InvalidBundle(format!(
            "bundle '{bundle_id}' has an inconsistent trailing content hash"
        )));
    }
    Ok(())
}

/// Forwards bundle bytes to `output` while hashing them, so a streamed bundle is
/// verified without reading it back. The last 32 bytes written are held back
/// from the hash because they are the trailing ID, not content.
pub(crate) struct BundleIdentityWriter<W> {
    output: W,
    hasher: blake3::Hasher,
    tail: Vec<u8>,
}

impl<W: Write> BundleIdentityWriter<W> {
    pub(crate) fn new(output: W) -> Self {
        Self {
            output,
            hasher: blake3::Hasher::new(),
            tail: Vec::with_capacity(2 * BUNDLE_ID_SIZE as usize),
        }
    }

    /// Flushes the output and checks the streamed bytes against `bundle_id`.
    pub(crate) fn finish(mut self, bundle_id: &str) -> Result<W> {
        self.output.flush()?;
        let expected_hash = ChunkHash::parse_hex(bundle_id)?;
        if self.tail.len() < BUNDLE_ID_SIZE as usize {
            return Err(crate::error::Error::InvalidBundle(format!(
                "bundle '{bundle_id}' is missing its content hash"
            )));
        }
        check_bundle_identity(
            bundle_id,
            expected_hash,
            ChunkHash::from_blake3(self.hasher.finalize()),
            &self.tail,
        )?;
        Ok(self.output)
    }
}

impl<W: Write> Write for BundleIdentityWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let written = self.output.write(bytes)?;
        let id_size = BUNDLE_ID_SIZE as usize;
        let bytes = &bytes[..written];
        if bytes.len() >= id_size {
            // Everything before the new last 32 bytes is content.
            self.hasher.update(&self.tail);
            self.hasher.update(&bytes[..bytes.len() - id_size]);
            self.tail.clear();
            self.tail.extend_from_slice(&bytes[bytes.len() - id_size..]);
        } else {
            self.tail.extend_from_slice(bytes);
            let excess = self.tail.len().saturating_sub(id_size);
            self.hasher.update(&self.tail[..excess]);
            self.tail.drain(..excess);
        }
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.output.flush()
    }
}

struct HashingReader<'a, R> {
    reader: R,
    hasher: &'a mut blake3::Hasher,
}
impl<R: Read> Read for HashingReader<'_, R> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        let count = self.reader.read(bytes)?;
        self.hasher.update(&bytes[..count]);
        Ok(count)
    }
}

fn stream_decoded_chunks<R, F>(
    reader: &mut R,
    entries: &[BundleTableEntry],
    required: &[bool],
    decompress: &mut Duration,
    profiler: Option<&crate::profile::GutchProfiler>,
    on_chunk: &mut F,
) -> Result<()>
where
    R: Read,
    F: FnMut(usize, &[u8]) -> Result<()>,
{
    let mut chunk_buffer = Vec::new();
    let mut discard_buffer = vec![0u8; STREAM_BUFFER_SIZE];

    for (index, entry) in entries.iter().enumerate() {
        let raw_size = usize::try_from(entry.raw_size).map_err(|_| {
            crate::error::Error::InvalidBundle(format!(
                "bundle chunk {} is too large for this platform",
                index
            ))
        })?;

        if required[index] {
            // Retain initialized bytes when a smaller chunk follows a larger one.
            // The decoder overwrites only the current chunk before it is delivered.
            if chunk_buffer.len() < raw_size {
                chunk_buffer.resize(raw_size, 0);
            }
            let chunk = &mut chunk_buffer[..raw_size];
            let decode_started_at = ProfileInstant::detailed(profiler);
            reader.read_exact(chunk)?;
            *decompress += decode_started_at.elapsed();
            on_chunk(index, chunk)?;
        } else {
            let mut remaining = raw_size;
            while remaining > 0 {
                let read_size = remaining.min(discard_buffer.len());
                let decode_started_at = ProfileInstant::detailed(profiler);
                reader.read_exact(&mut discard_buffer[..read_size])?;
                *decompress += decode_started_at.elapsed();
                remaining -= read_size;
            }
        }
    }

    let mut trailing = [0u8; 1];
    let decode_started_at = ProfileInstant::detailed(profiler);
    let trailing_bytes = reader.read(&mut trailing)?;
    *decompress += decode_started_at.elapsed();
    if trailing_bytes != 0 {
        return Err(crate::error::Error::InvalidBundle(
            "bundle payload contains decoded bytes after its table entries".into(),
        ));
    }
    Ok(())
}

fn assign_compressed_size_estimates(
    entries: &mut [BundleTableEntry],
    compressed_payload_size: u64,
) -> Result<()> {
    if entries.is_empty() {
        return Ok(());
    }

    let total_raw_size = entries.iter().try_fold(0u64, |total, entry| {
        total
            .checked_add(u64::from(entry.raw_size))
            .ok_or_else(|| crate::error::Error::InvalidBundle("bundle raw size overflow".into()))
    })?;
    let mut assigned = 0u64;
    let entry_count = entries.len();
    for (index, entry) in entries.iter_mut().enumerate() {
        let remaining = compressed_payload_size.saturating_sub(assigned);
        let compressed_size = if index + 1 == entry_count {
            remaining
        } else if total_raw_size == 0 {
            0
        } else {
            ((entry.raw_size as u128 * compressed_payload_size as u128) / total_raw_size as u128)
                as u64
        };
        entry.compressed_size = format::size_u32(compressed_size)?;
        assigned = assigned.checked_add(compressed_size).ok_or_else(|| {
            crate::error::Error::InvalidBundle("bundle compressed size overflow".into())
        })?;
    }
    if assigned != compressed_payload_size {
        return Err(crate::error::Error::InvalidBundle(
            "bundle compressed-size estimates do not cover its payload".into(),
        ));
    }
    Ok(())
}

fn validate_bundle_entries(entries: &[BundleTableEntry]) -> Result<()> {
    let mut total = 0u64;
    let mut hashes = std::collections::HashSet::with_capacity(entries.len());
    for entry in entries {
        if entry.raw_size == 0
            || u64::from(entry.raw_size) > format::MAX_RAW_CHUNK_SIZE
            || !hashes.insert(&entry.chunk_hash)
        {
            return Err(crate::Error::bundle("invalid or duplicate bundle chunk"));
        }
        total = total
            .checked_add(u64::from(entry.raw_size))
            .ok_or_else(|| crate::Error::bundle("bundle size overflow"))?;
        if total > format::MAX_RAW_BUNDLE_SIZE {
            return Err(crate::Error::bundle("bundle exceeds raw size limit"));
        }
    }
    Ok(())
}

fn write_bundle_table_entries(writer: &mut impl Write, entries: &[BundleTableEntry]) -> Result<()> {
    for entry in entries {
        writer.write_all(entry.chunk_hash.as_bytes())?;
    }
    for entry in entries {
        writer.write_all(&entry.raw_size.to_le_bytes())?;
    }
    Ok(())
}

fn read_bundle_table_entries(
    reader: &mut impl Read,
    chunk_count: usize,
    table_len: usize,
) -> Result<Vec<BundleTableEntry>> {
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
    let chunk_hashes: Vec<_> = (0..chunk_count)
        .map(|_| read_exact_array(reader).map(ChunkHash::from_bytes))
        .collect::<Result<_>>()?;
    let mut raw_offset = 0u64;
    for chunk_hash in chunk_hashes {
        let raw_size = u64::from(read_u32(reader)?);
        let compressed_size = 0;
        entries.push(BundleTableEntry {
            chunk_hash,
            raw_size: format::size_u32(raw_size)?,
            compressed_size,
            raw_offset: format::size_u32(raw_offset)?,
        });
        raw_offset = raw_offset
            .checked_add(raw_size)
            .ok_or_else(|| crate::Error::bundle("bundle raw offset overflow"))?;
    }
    Ok(entries)
}

#[cfg(test)]
pub(crate) fn read_bundle<'a>(
    data: &'a [u8],
    compression_profile: &CompressionProfile,
) -> Result<BundleReader<std::io::Cursor<&'a [u8]>>> {
    let cursor = std::io::Cursor::new(data);
    BundleReader::open(cursor, compression_profile)
}

#[cfg(test)]
#[path = "bundle_tests.rs"]
mod tests;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BundleTableEntry {
    pub chunk_hash: ChunkHash,
    pub raw_size: u32,
    pub compressed_size: u32,
    #[serde(default)]
    pub raw_offset: u32,
}

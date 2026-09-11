#[cfg(test)]
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::io::Seek;
use std::io::SeekFrom;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use crate::artifact::compress::validate_profile;
use crate::artifact::hash;
use crate::artifact::hash::ChunkHash;
use crate::error::Result;
use crate::manifest::BundleTableEntry;
use crate::manifest::CompressionKind;
use crate::manifest::CompressionProfile;
use crate::manifest::CompressionUnit;

use crate::artifact::format;
use crate::artifact::format::BUNDLE_FOOTER_MAGIC as FOOTER_MAGIC;
use crate::artifact::format::BUNDLE_MAGIC;
const BUNDLE_HEADER_SIZE: u64 = 16 + 2 + 2 + 4;
const BUNDLE_TRAILER_SIZE: u64 = 4 + 16;
const BUNDLE_ID_SIZE: u64 = 64;
const BUNDLE_TABLE_ENTRY_SIZE: usize = 36;
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

pub(crate) struct BundleBuilder {
    chunks: Vec<ChunkData>,
    pub(crate) profile: CompressionProfile,
}

struct ChunkData {
    hash: ChunkHash,
    raw_size: u64,
    raw: Option<ChunkRawData>,
}

enum ChunkRawData {
    #[cfg(test)]
    Owned(Vec<u8>),
    Shared(Arc<[u8]>),
}

impl ChunkRawData {
    fn as_slice(&self) -> &[u8] {
        match self {
            #[cfg(test)]
            Self::Owned(bytes) => bytes.as_slice(),
            Self::Shared(bytes) => bytes.as_ref(),
        }
    }
}

impl BundleBuilder {
    pub(crate) fn new(profile: CompressionProfile) -> Self {
        Self {
            chunks: Vec::new(),
            profile,
        }
    }

    #[cfg(test)]
    pub(crate) fn add_chunk(&mut self, chunk_hash: ChunkHash, raw_data: Vec<u8>) {
        let raw_size = raw_data.len() as u64;
        self.chunks.push(ChunkData {
            hash: chunk_hash,
            raw_size,
            raw: Some(ChunkRawData::Owned(raw_data)),
        });
    }

    pub(crate) fn add_shared_chunk(&mut self, chunk_hash: ChunkHash, raw_data: Arc<[u8]>) {
        let raw_size = raw_data.len() as u64;
        self.chunks.push(ChunkData {
            hash: chunk_hash,
            raw_size,
            raw: Some(ChunkRawData::Shared(raw_data)),
        });
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }

    #[cfg(test)]
    #[cfg(test)]
    pub(crate) fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    /// Test-only convenience. Production capture writes to a staged file.
    #[cfg(test)]
    pub(crate) fn build(&self) -> Result<Vec<u8>> {
        let (_, bundle) = self.build_with_id()?;
        Ok(bundle)
    }

    /// Test-only convenience. Production capture writes to a staged file.
    #[cfg(test)]
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
        if self.profile.compression_unit != CompressionUnit::Bundle {
            return Err(crate::error::Error::InvalidBundle(
                "gutch only supports bundle-compressed bundles".into(),
            ));
        }
        crate::filesystem::create_dir_all(staging_root)?;

        let temporary = tempfile::NamedTempFile::new_in(staging_root)?;
        let capture = {
            let mut file = temporary.reopen()?;
            let capture = self.write_bundle(&mut file, profiler)?;
            let _flush_span = profiler.map(|profiler| {
                profiler.span_with_args(
                    "capture.compress_bundle_artifact.flush_staged_file",
                    std::collections::BTreeMap::from([(
                        "bundle_size_bytes".to_string(),
                        serde_json::json!(capture.size),
                    )]),
                )
            });
            file.flush()?;
            drop(_flush_span);
            let _sync_span = profiler.map(|profiler| {
                profiler.span_with_args(
                    "capture.compress_bundle_artifact.sync_staged_file",
                    std::collections::BTreeMap::from([(
                        "bundle_size_bytes".to_string(),
                        serde_json::json!(capture.size),
                    )]),
                )
            });
            file.sync_all()?;
            drop(_sync_span);
            capture
        };
        let _keep_span = profiler
            .map(|profiler| profiler.span("capture.compress_bundle_artifact.keep_staged_file"));
        let (_, staged_path) = temporary
            .keep()
            .map_err(|error| crate::error::Error::Io(error.error))?;
        drop(_keep_span);
        if let Some(profiler) = profiler {
            profiler.event_with_args(
                "capture.compress_bundle_artifact.keep_staged_file.complete",
                std::collections::BTreeMap::from([
                    (
                        "bundle_size_bytes".to_string(),
                        serde_json::json!(capture.size),
                    ),
                    (
                        "id_present".to_string(),
                        serde_json::json!(!capture.bundle_id.is_empty()),
                    ),
                    (
                        "next_phase".to_string(),
                        serde_json::json!("return_staged_artifact"),
                    ),
                ]),
            );
        }

        Ok(BundleBuildArtifact {
            staged_path,
            ..capture
        })
    }

    fn write_bundle(
        &self,
        file: &mut File,
        profiler: Option<&crate::profile::GutchProfiler>,
    ) -> Result<BundleBuildArtifact> {
        let chunk_count = u32::try_from(self.chunks.len()).map_err(|_| {
            crate::error::Error::InvalidBundle("bundle contains too many chunks".into())
        })?;
        let mut hashing = HashingWriter::new(file);
        hashing.write_all(BUNDLE_MAGIC)?;
        hashing.write_all(&format::VERSION.to_le_bytes())?;
        format::write_codec(&mut hashing, self.profile.compression)?;
        hashing.write_all(&chunk_count.to_le_bytes())?;

        let mut entries = Vec::with_capacity(self.chunks.len());
        let mut raw_offset = 0u64;
        for chunk in &self.chunks {
            let raw = chunk.raw.as_ref().ok_or_else(|| {
                crate::error::Error::InvalidBundle(
                    "bundle-compressed chunks must retain raw bytes until bundle capture".into(),
                )
            })?;
            if raw.as_slice().len() as u64 != chunk.raw_size {
                return Err(crate::error::Error::InvalidBundle(
                    "bundle chunk raw size changed before writing".into(),
                ));
            }
            entries.push(BundleTableEntry {
                chunk_hash: chunk.hash.to_hex(),
                raw_size: format::size_u32(chunk.raw_size)?,
                compressed_size: 0,
                raw_offset: format::size_u32(raw_offset)?,
            });
            raw_offset = raw_offset.checked_add(chunk.raw_size).ok_or_else(|| {
                crate::error::Error::InvalidBundle("bundle raw size overflow".into())
            })?;
        }

        validate_bundle_entries(&entries, 0, 0)?;
        let compression_started_at = Instant::now();
        let _payload_span = profiler.map(|profiler| {
            profiler.span_with_args(
                "capture.compress_bundle_artifact.compress_payload",
                std::collections::BTreeMap::from([(
                    "chunk_count".to_string(),
                    serde_json::json!(self.chunks.len()),
                )]),
            )
        });
        let raw_segments: Vec<&[u8]> = self
            .chunks
            .iter()
            .map(|chunk| {
                chunk
                    .raw
                    .as_ref()
                    .expect("bundle entries validated above")
                    .as_slice()
            })
            .collect();
        let raw_size = self.chunks.iter().try_fold(0u64, |total, chunk| {
            total.checked_add(chunk.raw_size).ok_or_else(|| {
                crate::error::Error::InvalidBundle("bundle raw size overflow".into())
            })
        })?;
        validate_profile(&self.profile)?;
        if raw_size > format::MAX_RAW_BUNDLE_SIZE {
            return Err(crate::Error::bundle("bundle exceeds raw size limit"));
        }
        crate::artifact::compress::encode_to_writer(
            &mut hashing,
            &self.profile,
            raw_size,
            |writer| {
                for segment in &raw_segments {
                    writer.write_all(segment)?;
                }
                Ok(())
            },
        )?;
        let compression_time = compression_started_at.elapsed();
        let payload_end = hashing.position()?;
        let compressed_output_bytes =
            payload_end.checked_sub(BUNDLE_HEADER_SIZE).ok_or_else(|| {
                crate::error::Error::InvalidBundle("bundle payload position underflow".into())
            })?;
        assign_compressed_size_estimates(&mut entries, compressed_output_bytes)?;
        drop(_payload_span);
        let serialization_started_at = Instant::now();
        let _footer_span = profiler.map(|profiler| {
            profiler.span_with_args(
                "capture.compress_bundle_artifact.write_footer",
                std::collections::BTreeMap::from([(
                    "chunk_count".to_string(),
                    serde_json::json!(entries.len()),
                )]),
            )
        });
        let mut footer = Vec::with_capacity(4 + entries.len() * BUNDLE_TABLE_ENTRY_SIZE);
        footer.extend_from_slice(&chunk_count.to_le_bytes());
        write_bundle_table_entries(&mut footer, &entries)?;
        let footer_length = u64::try_from(footer.len())
            .map_err(|_| crate::error::Error::InvalidBundle("bundle footer is too large".into()))?;
        hashing.write_all(&footer)?;
        hashing.write_all(&format::size_u32(footer_length)?.to_le_bytes())?;
        hashing.write_all(FOOTER_MAGIC)?;
        let bytes_without_id = hashing.position()?;

        drop(_footer_span);
        let _finalize_span = profiler.map(|profiler| {
            profiler.span_with_args(
                "capture.compress_bundle_artifact.finalize_id",
                std::collections::BTreeMap::from([
                    ("chunk_count".to_string(), serde_json::json!(chunk_count)),
                    (
                        "hash_input_bytes".to_string(),
                        serde_json::json!(bytes_without_id),
                    ),
                    (
                        "payload_compressed_bytes".to_string(),
                        serde_json::json!(compressed_output_bytes),
                    ),
                    ("footer_bytes".to_string(), serde_json::json!(footer.len())),
                ]),
            )
        });
        let HashingWriter { file, hasher } = hashing;
        let bundle_id = hash::hash_hex(&hasher.finalize());
        file.write_all(bundle_id.as_bytes())?;
        let bundle_size = bytes_without_id
            .checked_add(BUNDLE_ID_SIZE)
            .ok_or_else(|| crate::error::Error::InvalidBundle("bundle size overflow".into()))?;
        format::size_u32(bundle_size)?;
        drop(_finalize_span);
        if let Some(profiler) = profiler {
            profiler.event_with_args(
                "capture.compress_bundle_artifact.finalize_id.complete",
                std::collections::BTreeMap::from([
                    (
                        "id_present".to_string(),
                        serde_json::json!(!bundle_id.is_empty()),
                    ),
                    ("id_bytes".to_string(), serde_json::json!(bundle_id.len())),
                    (
                        "bundle_size_bytes".to_string(),
                        serde_json::json!(bundle_size),
                    ),
                    (
                        "next_phase".to_string(),
                        serde_json::json!("flush_staged_file"),
                    ),
                ]),
            );
        }

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
    chunk_hashes: Vec<ChunkHash>,
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
        let (mut entries, chunk_hashes) =
            read_bundle_table_entries(&mut footer_reader, chunk_count, table_length)?;
        if footer_reader.position() != footer_length {
            return Err(crate::error::Error::InvalidBundle(
                "bundle footer contains trailing bytes".into(),
            ));
        }
        assign_compressed_size_estimates(&mut entries, footer_start - data_start)?;
        validate_bundle_entries(&entries, data_start, footer_start)?;

        #[cfg(test)]
        let chunk_index_by_hash = chunk_hashes
            .iter()
            .enumerate()
            .map(|(index, chunk_hash)| (*chunk_hash, index))
            .collect();
        reader.seek(SeekFrom::Start(data_start))?;
        Ok(Self {
            entries,
            chunk_hashes,
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

        let read_started_at = Instant::now();
        let _read_span = profiler.map(|profiler| {
            profiler.span_with_args(
                "snapshot.reconstruct.bundle.read_payload",
                bundle_payload_trace_args(compressed_size, expected_raw_size),
            )
        });
        self.reader.seek(SeekFrom::Start(0))?;
        let mut hasher = blake3::Hasher::new();
        let mut header = [0u8; BUNDLE_HEADER_SIZE as usize];
        self.reader.read_exact(&mut header)?;
        hasher.update(&header);
        let read = read_started_at.elapsed();
        drop(_read_span);

        let _decompress_span = profiler.map(|profiler| {
            profiler.span_with_args(
                "snapshot.reconstruct.bundle.decompress_payload",
                bundle_payload_trace_args(compressed_size, expected_raw_size),
            )
        });
        let mut decompress = Duration::default();
        let mut encoded = HashingReader {
            reader: (&mut self.reader).take(compressed_size),
            hasher: &mut hasher,
        };
        let mut decoder =
            crate::artifact::compress::decoder_from_reader(&mut encoded, self.compression)?;
        let stream_result = stream_decoded_chunks(
            &mut decoder,
            &self.entries,
            required,
            &mut decompress,
            &mut on_chunk,
        );
        stream_result?;
        drop(decoder);
        std::io::copy(&mut encoded, &mut std::io::sink())?;
        if encoded.reader.limit() != 0 {
            return Err(crate::Error::bundle("truncated bundle payload"));
        }
        drop(encoded);
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
        let trailer = read_exact_array::<64>(&mut self.reader)?;
        if trailer != *hasher.finalize().to_hex().as_bytes() {
            return Err(crate::Error::bundle("bundle content identity mismatch"));
        }
        // Keep the timing metric focused on decoder reads. The callback can
        // perform hashing and positional output writes, which are accounted
        // for separately by reconstruction timings.
        drop(_decompress_span);

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
        if read_exact_array::<64>(&mut self.reader)?.as_slice() != expected.to_hex().as_bytes() {
            return Err(crate::Error::bundle(
                "bundle filename and trailing identity disagree",
            ));
        }
        self.verify()?;
        Ok(())
    }

    pub(crate) fn verify(&mut self) -> Result<bool> {
        let hashes = self.chunk_hashes.clone();
        let required = vec![true; hashes.len()];
        self.stream_bundle_chunks_with_profiler(None, &required, |index, bytes| {
            let actual = ChunkHash::from_blake3(hash::hash_chunk(bytes));
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
/// bytes before the trailing 64-byte hex ID must equal the expected ID, and
/// the trailing bytes must repeat it. Shared by local verification and
/// remote download so both reject the same identity failures.
pub(crate) fn verify_bundle_identity_bytes(bundle_id: &str, bytes: &[u8]) -> Result<()> {
    let expected_hash = ChunkHash::parse_hex(bundle_id)?;
    let id_size = BUNDLE_ID_SIZE as usize;
    if bytes.len() < id_size {
        return Err(crate::error::Error::InvalidBundle(format!(
            "bundle '{bundle_id}' is missing its content hash"
        )));
    }
    let actual_hash = hash::hash_hex(&blake3::hash(&bytes[..bytes.len() - id_size]));
    if actual_hash != expected_hash.to_hex() {
        return Err(crate::error::Error::HashMismatch {
            expected: expected_hash.to_hex(),
            actual: actual_hash,
        });
    }
    let expected_hex = expected_hash.to_hex();
    if &bytes[bytes.len() - id_size..] != expected_hex.as_bytes() {
        return Err(crate::error::Error::InvalidBundle(format!(
            "bundle '{bundle_id}' has an inconsistent trailing content hash"
        )));
    }
    Ok(())
}

/// Verifies complete bundle identity for a stored bundle file without
/// loading it: the path-derived ID, the trailing ID, and the BLAKE3 of the
/// pre-ID bytes must all match the expected manifest ID.
pub(crate) fn verify_bundle_identity_file(path: &Path, bundle_id: &str) -> Result<()> {
    let expected_hash = ChunkHash::parse_hex(bundle_id)?;
    let path_id = crate::repo::bundle_id_from_bundle_path(path).ok_or_else(|| {
        crate::error::Error::InvalidBundle(format!(
            "bundle path '{}' does not name a bundle ID",
            path.display()
        ))
    })?;
    if path_id != expected_hash.to_hex() {
        return Err(crate::error::Error::HashMismatch {
            expected: expected_hash.to_hex(),
            actual: path_id,
        });
    }
    verify_bundle_identity_reader(&mut File::open(path)?, bundle_id)
}

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
    let mut buffer = vec![0u8; 1024 * 1024];
    while remaining > 0 {
        let take = remaining.min(buffer.len() as u64) as usize;
        file.read_exact(&mut buffer[..take])?;
        hasher.update(&buffer[..take]);
        remaining -= take as u64;
    }
    let mut trailing = vec![0u8; id_size];
    file.read_exact(&mut trailing)?;
    let actual_hash = hash::hash_hex(&hasher.finalize());
    if actual_hash != expected_hash.to_hex() {
        return Err(crate::error::Error::HashMismatch {
            expected: expected_hash.to_hex(),
            actual: actual_hash,
        });
    }
    if trailing != expected_hash.to_hex().as_bytes() {
        return Err(crate::error::Error::InvalidBundle(format!(
            "bundle '{bundle_id}' has an inconsistent trailing content hash"
        )));
    }
    Ok(())
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
            chunk_buffer.resize(raw_size, 0);
            let decode_started_at = Instant::now();
            reader.read_exact(&mut chunk_buffer)?;
            *decompress += decode_started_at.elapsed();
            on_chunk(index, &chunk_buffer)?;
        } else {
            let mut remaining = raw_size;
            while remaining > 0 {
                let read_size = remaining.min(discard_buffer.len());
                let decode_started_at = Instant::now();
                reader.read_exact(&mut discard_buffer[..read_size])?;
                *decompress += decode_started_at.elapsed();
                remaining -= read_size;
            }
        }
    }

    let mut trailing = [0u8; 1];
    let decode_started_at = Instant::now();
    let trailing_bytes = reader.read(&mut trailing)?;
    *decompress += decode_started_at.elapsed();
    if trailing_bytes != 0 {
        return Err(crate::error::Error::InvalidBundle(
            "bundle payload contains decoded bytes after its table entries".into(),
        ));
    }
    Ok(())
}

fn bundle_payload_trace_args(
    compressed_size: u64,
    raw_size: u64,
) -> std::collections::BTreeMap<String, serde_json::Value> {
    std::collections::BTreeMap::from([
        (
            "compressed_payload_mib".to_string(),
            serde_json::json!(compressed_size as f64 / (1024.0 * 1024.0)),
        ),
        (
            "decoded_payload_mib".to_string(),
            serde_json::json!(raw_size as f64 / (1024.0 * 1024.0)),
        ),
    ])
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

fn validate_bundle_entries(
    entries: &[BundleTableEntry],
    _data_start: u64,
    _payload_end: u64,
) -> Result<()> {
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
        writer.write_all(ChunkHash::parse_hex(&entry.chunk_hash)?.as_bytes())?;
        writer.write_all(&format::size_u32(entry.raw_size)?.to_le_bytes())?;
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
    let mut raw_offset = 0u64;
    for _ in 0..chunk_count {
        let chunk_hash = ChunkHash::from_bytes(read_exact_array(reader)?);
        let raw_size = u64::from(read_u32(reader)?);
        let compressed_size = 0;
        chunk_hashes.push(chunk_hash);
        entries.push(BundleTableEntry {
            chunk_hash: chunk_hash.to_hex(),
            raw_size: format::size_u32(raw_size)?,
            compressed_size,
            raw_offset: format::size_u32(raw_offset)?,
        });
        raw_offset = raw_offset
            .checked_add(raw_size)
            .ok_or_else(|| crate::Error::bundle("bundle raw offset overflow"))?;
    }
    Ok((entries, chunk_hashes))
}

fn read_exact_array<const N: usize>(reader: &mut impl Read) -> Result<[u8; N]> {
    let mut bytes = [0u8; N];
    reader.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn read_u32(reader: &mut impl Read) -> Result<u32> {
    Ok(u32::from_le_bytes(read_exact_array(reader)?))
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

/// Inspect a bundle using its repository's compression profile without rendering output.
pub fn inspect_bundle(bundle: &Path, repo: &Path) -> Result<Vec<BundleTableEntry>> {
    let (_, manifest) = crate::repo::paths::load_gutch_repo_manifest(repo)?;
    let reader = BundleReader::open(File::open(bundle)?, &manifest.compression_profile())?;
    Ok(reader.entries().to_vec())
}

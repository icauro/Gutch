#[cfg(test)]
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::error::Result;
use crate::hash::{self, ChunkHash};
use crate::manifest::{BundleTableEntry, CompressionKind, CompressionProfile, CompressionUnit};

const BUNDLE_MAGIC: &[u8; 8] = b"ZNRBNDL\0";
const FOOTER_MAGIC: &[u8; 8] = b"ZNRFOOT\0";
const BUNDLE_HEADER_SIZE: u64 = 8 + 4;
const BUNDLE_TRAILER_SIZE: u64 = 8 + 8;
const BUNDLE_ID_SIZE: u64 = 64;
const BUNDLE_TABLE_ENTRY_SIZE: usize = 32 + (8 * 4);
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
        profiler: Option<&crate::GutchProfiler>,
    ) -> Result<BundleBuildArtifact> {
        if self.profile.compression_unit != CompressionUnit::Bundle {
            return Err(crate::error::Error::InvalidBundle(
                "gutch only supports bundle-compressed bundles".into(),
            ));
        }
        crate::helpers::create_dir_all(staging_root)?;

        let temporary = tempfile::NamedTempFile::new_in(staging_root)?;
        let build = {
            let mut file = temporary.reopen()?;
            let build = self.write_bundle(&mut file, profiler)?;
            let _flush_span = profiler.map(|profiler| {
                profiler.span_with_args(
                    "capture.compress_bundle_artifact.flush_staged_file",
                    std::collections::BTreeMap::from([(
                        "bundle_size_bytes".to_string(),
                        serde_json::json!(build.size),
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
                        serde_json::json!(build.size),
                    )]),
                )
            });
            file.sync_all()?;
            drop(_sync_span);
            build
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
                        serde_json::json!(build.size),
                    ),
                    (
                        "id_present".to_string(),
                        serde_json::json!(!build.bundle_id.is_empty()),
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
            ..build
        })
    }

    fn write_bundle(
        &self,
        file: &mut File,
        profiler: Option<&crate::GutchProfiler>,
    ) -> Result<BundleBuildArtifact> {
        let chunk_count = u32::try_from(self.chunks.len()).map_err(|_| {
            crate::error::Error::InvalidBundle("bundle contains too many chunks".into())
        })?;
        let mut hashing = HashingWriter::new(file);
        hashing.write_all(BUNDLE_MAGIC)?;
        hashing.write_all(&chunk_count.to_le_bytes())?;

        let mut entries = Vec::with_capacity(self.chunks.len());
        let mut raw_offset = 0u64;
        for chunk in &self.chunks {
            let raw = chunk.raw.as_ref().ok_or_else(|| {
                crate::error::Error::InvalidBundle(
                    "bundle-compressed chunks must retain raw bytes until bundle build".into(),
                )
            })?;
            if raw.as_slice().len() as u64 != chunk.raw_size {
                return Err(crate::error::Error::InvalidBundle(
                    "bundle chunk raw size changed before writing".into(),
                ));
            }
            entries.push(BundleTableEntry {
                chunk_hash: chunk.hash.to_hex(),
                raw_size: chunk.raw_size,
                compressed_size: 0,
                compressed_offset: 0,
                raw_offset,
            });
            raw_offset = raw_offset.checked_add(chunk.raw_size).ok_or_else(|| {
                crate::error::Error::InvalidBundle("bundle raw size overflow".into())
            })?;
        }

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
        match self.profile.compression {
            CompressionKind::None => {
                for chunk in &self.chunks {
                    let raw = chunk.raw.as_ref().expect("bundle entries validated above");
                    hashing.write_all(raw.as_slice())?;
                }
            }
            CompressionKind::Zstd if !self.chunks.is_empty() => {
                let mut encoder = zstd::stream::Encoder::new(&mut hashing, self.profile.zstd_level)
                    .map_err(|error| {
                        crate::error::Error::bundle(format!("zstd compress: {error}"))
                    })?;
                for chunk in &self.chunks {
                    let raw = chunk.raw.as_ref().expect("bundle entries validated above");
                    encoder.write_all(raw.as_slice()).map_err(|error| {
                        crate::error::Error::bundle(format!("zstd compress: {error}"))
                    })?;
                }
                encoder.finish().map_err(|error| {
                    crate::error::Error::bundle(format!("zstd compress: {error}"))
                })?;
            }
            CompressionKind::Zstd => {}
        }
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
        hashing.write_all(&footer_length.to_le_bytes())?;
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
    pub(crate) fn open(mut reader: R, compression_profile: &CompressionProfile) -> Result<Self> {
        if compression_profile.compression_unit != CompressionUnit::Bundle {
            return Err(crate::error::Error::InvalidBundle(
                "gutch only supports bundle-compressed bundles".into(),
            ));
        }

        let end = reader.seek(SeekFrom::End(0))?;
        let minimum_size = BUNDLE_HEADER_SIZE + BUNDLE_TRAILER_SIZE + BUNDLE_ID_SIZE;
        if end < minimum_size {
            return Err(crate::error::Error::InvalidBundle(
                "bundle is smaller than its fixed framing".into(),
            ));
        }
        reader.seek(SeekFrom::Start(0))?;
        if &read_exact_array::<8>(&mut reader)? != BUNDLE_MAGIC {
            return Err(crate::error::Error::InvalidBundle(
                "bundle magic is invalid".into(),
            ));
        }
        let chunk_count = read_u32(&mut reader)? as usize;
        let data_start = BUNDLE_HEADER_SIZE;

        reader.seek(SeekFrom::Start(end - BUNDLE_ID_SIZE - BUNDLE_TRAILER_SIZE))?;
        let footer_length = read_u64(&mut reader)?;
        if &read_exact_array::<8>(&mut reader)? != FOOTER_MAGIC {
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
        let (entries, chunk_hashes) =
            read_bundle_table_entries(&mut footer_reader, chunk_count, table_length)?;
        if footer_reader.position() != footer_length {
            return Err(crate::error::Error::InvalidBundle(
                "bundle footer contains trailing bytes".into(),
            ));
        }
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
            compression: compression_profile.compression,
            #[cfg(test)]
            compression_unit: compression_profile.compression_unit,
            reader,
        })
    }

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
    pub(crate) fn read_bundle_payload(&mut self) -> Result<(Vec<u8>, BundleReadMetrics)> {
        self.read_bundle_payload_with_profiler(None)
    }

    pub(crate) fn read_bundle_payload_with_profiler(
        &mut self,
        profiler: Option<&crate::GutchProfiler>,
    ) -> Result<(Vec<u8>, BundleReadMetrics)> {
        let required = vec![true; self.entries.len()];
        let expected_raw_size = self.entries.iter().try_fold(0u64, |total, entry| {
            total.checked_add(entry.raw_size).ok_or_else(|| {
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
        profiler: Option<&crate::GutchProfiler>,
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
        let compressed_size_usize = usize::try_from(compressed_size).map_err(|_| {
            crate::error::Error::InvalidBundle(
                "bundle payload is too large for this platform".into(),
            )
        })?;
        let expected_raw_size = self.entries.iter().try_fold(0u64, |total, entry| {
            total.checked_add(entry.raw_size).ok_or_else(|| {
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
        self.reader.seek(SeekFrom::Start(self.data_start))?;
        let mut compressed = vec![0u8; compressed_size_usize];
        self.reader.read_exact(&mut compressed)?;
        let read = read_started_at.elapsed();
        drop(_read_span);

        let _decompress_span = profiler.map(|profiler| {
            profiler.span_with_args(
                "snapshot.reconstruct.bundle.decompress_payload",
                bundle_payload_trace_args(compressed_size, expected_raw_size),
            )
        });
        let mut decompress = Duration::default();
        let stream_result = match self.compression {
            CompressionKind::None => stream_decoded_chunks(
                &mut std::io::Cursor::new(compressed.as_slice()),
                &self.entries,
                required,
                &mut decompress,
                &mut on_chunk,
            ),
            CompressionKind::Zstd => {
                let mut decoder =
                    zstd::stream::read::Decoder::new(std::io::Cursor::new(compressed.as_slice()))
                        .map_err(|error| {
                        crate::error::Error::bundle(format!("zstd decompress: {error}"))
                    })?;
                stream_decoded_chunks(
                    &mut decoder,
                    &self.entries,
                    required,
                    &mut decompress,
                    &mut on_chunk,
                )
            }
        };
        stream_result?;
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

    pub(crate) fn verify(&mut self) -> Result<bool> {
        let (bytes, _) = self.read_bundle_payload()?;
        for (index, entry) in self.entries.iter().enumerate() {
            let raw_offset = usize::try_from(entry.raw_offset).map_err(|_| {
                crate::error::Error::InvalidBundle("bundle raw offset is too large".into())
            })?;
            let raw_size = usize::try_from(entry.raw_size).map_err(|_| {
                crate::error::Error::InvalidBundle("bundle raw size is too large".into())
            })?;
            let raw_end = raw_offset.checked_add(raw_size).ok_or_else(|| {
                crate::error::Error::InvalidBundle("bundle raw size overflow".into())
            })?;
            let actual_hash = ChunkHash::from_blake3(hash::hash_chunk(&bytes[raw_offset..raw_end]));
            if actual_hash != self.chunk_hashes[index] {
                return Err(crate::error::Error::HashMismatch {
                    expected: self.chunk_hashes[index].to_hex(),
                    actual: actual_hash.to_hex(),
                });
            }
        }
        Ok(true)
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
        if compressed_payload_size != 0 {
            return Err(crate::error::Error::InvalidBundle(
                "empty bundle has a non-empty payload".into(),
            ));
        }
        return Ok(());
    }

    let total_raw_size = entries.iter().try_fold(0u64, |total, entry| {
        total
            .checked_add(entry.raw_size)
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
        entry.compressed_size = compressed_size;
        entry.compressed_offset = 0;
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
    data_start: u64,
    payload_end: u64,
) -> Result<()> {
    let payload_size = payload_end.checked_sub(data_start).ok_or_else(|| {
        crate::error::Error::InvalidBundle("bundle payload position underflow".into())
    })?;
    let mut expected_raw_offset = 0u64;
    let mut compressed_size_estimate = 0u64;
    for (index, entry) in entries.iter().enumerate() {
        if entry.raw_offset != expected_raw_offset {
            return Err(crate::error::Error::InvalidBundle(format!(
                "bundle chunk {} starts at raw offset {}, expected {}",
                index, entry.raw_offset, expected_raw_offset
            )));
        }
        if entry.compressed_offset != 0 {
            return Err(crate::error::Error::InvalidBundle(format!(
                "bundle chunk {} has compressed offset {}, expected 0 for a bundle frame",
                index, entry.compressed_offset
            )));
        }
        expected_raw_offset = expected_raw_offset
            .checked_add(entry.raw_size)
            .ok_or_else(|| crate::error::Error::InvalidBundle("bundle raw size overflow".into()))?;
        compressed_size_estimate = compressed_size_estimate
            .checked_add(entry.compressed_size)
            .ok_or_else(|| {
                crate::error::Error::InvalidBundle("bundle compressed size overflow".into())
            })?;
    }
    if compressed_size_estimate != payload_size {
        return Err(crate::error::Error::InvalidBundle(format!(
            "bundle compressed-size estimates cover {}, payload is {} bytes",
            compressed_size_estimate, payload_size
        )));
    }
    Ok(())
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

fn read_u32(reader: &mut impl Read) -> Result<u32> {
    Ok(u32::from_le_bytes(read_exact_array(reader)?))
}

fn read_u64(reader: &mut impl Read) -> Result<u64> {
    Ok(u64::from_le_bytes(read_exact_array(reader)?))
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

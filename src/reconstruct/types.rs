use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tempfile::TempDir;

use crate::Result;
use crate::hash::ChunkHash;
use crate::manifest::runtime as manifest_runtime;
use crate::manifest::snapshot::SnapshotManifest;
use crate::manifest::{CompressionUnit, FileManifest};
use crate::runtime::{RuntimePlan, RuntimeResourceHints};

#[derive(Debug, Clone, Default)]
pub struct ReconstructTimings {
    pub runtime_plan: Option<RuntimePlan>,
    pub verify_chunks: bool,
    pub verify_files: bool,
    pub prepare_wall: Duration,
    pub reconstruct_wall: Duration,
    pub bundle_read: Duration,
    pub bundle_read_compressed_bytes: u64,
    pub bundle_read_decompressed_bytes: u64,
    pub chunk_verify: Duration,
    pub chunk_verify_bytes: u64,
    pub file_rebuild: Duration,
    pub file_rebuild_bytes: u64,
    pub file_verify: Duration,
    pub file_verify_bytes: u64,
    pub file_write: Duration,
    pub file_write_bytes: u64,
    pub total: Duration,
}

impl ReconstructTimings {
    pub(super) fn accumulate(&mut self, other: &Self) {
        self.bundle_read += other.bundle_read;
        self.bundle_read_compressed_bytes += other.bundle_read_compressed_bytes;
        self.bundle_read_decompressed_bytes += other.bundle_read_decompressed_bytes;
        self.chunk_verify += other.chunk_verify;
        self.chunk_verify_bytes += other.chunk_verify_bytes;
        self.file_rebuild += other.file_rebuild;
        self.file_rebuild_bytes += other.file_rebuild_bytes;
        self.file_verify += other.file_verify;
        self.file_verify_bytes += other.file_verify_bytes;
        self.file_write += other.file_write;
        self.file_write_bytes += other.file_write_bytes;
    }
}

#[derive(Clone, Copy)]
pub struct VerifiedSourceTree<'a> {
    pub snapshot_manifest: &'a SnapshotManifest,
    pub destination_dir: &'a Path,
}

#[derive(Debug, Clone, Copy)]
pub struct ReconstructOptions {
    pub verify_chunks: bool,
    pub verify_files: bool,
}

pub struct ReconstructRequest<'a> {
    pub snapshot_manifest: &'a SnapshotManifest,
    pub bundle_paths: &'a [PathBuf],
    pub compression_profile: &'a crate::manifest::CompressionProfile,
    pub verified_source: Option<VerifiedSourceTree<'a>>,
    pub output_dir: &'a Path,
    pub runtime_hints: &'a RuntimeResourceHints,
    pub options: ReconstructOptions,
    pub control: &'a crate::progress::GutchOperationControl,
}

impl Default for ReconstructOptions {
    fn default() -> Self {
        Self {
            verify_chunks: true,
            verify_files: true,
        }
    }
}

#[derive(Clone)]
pub(super) struct ChunkLocation {
    pub bundle_path: PathBuf,
    pub index: usize,
    pub compressed_size: u64,
    pub raw_offset: u64,
    pub raw_size: u64,
}

#[derive(Clone)]
pub(super) struct ChunkWriteSnapshot {
    pub output_path: PathBuf,
    pub file_offset: u64,
    pub raw_size: u64,
}

#[derive(Clone)]
pub(super) struct BundleScanInfo {
    pub compression_unit: CompressionUnit,
    pub raw_payload_size: u64,
}

#[derive(Clone)]
pub(super) enum BundleAccessStrategy {
    DirectChunkRead,
    StagedBundleRaw { staged_raw_path: PathBuf },
    StagedBundleMemory { raw_payload: Arc<[u8]> },
}

pub(super) enum StagedBundlePayload {
    TempFile(PathBuf),
    Memory(Arc<[u8]>),
}

pub(super) struct BundleAccessPlan {
    pub strategies: HashMap<PathBuf, BundleAccessStrategy>,
    pub _staging_dir: Option<TempDir>,
}

#[derive(Debug)]
pub(super) struct LoadedChunk {
    pub chunk_hash: ChunkHash,
    pub data: Vec<u8>,
    pub read_duration: Duration,
    pub chunk_verify_duration: Duration,
    pub compressed_bytes: u64,
    pub decompressed_bytes: u64,
}

pub(super) type TypedChunkRef<'a> = manifest_runtime::TypedChunkRef<'a>;

pub(super) fn typed_chunk_refs(file_manifest: &FileManifest) -> Result<Vec<TypedChunkRef<'_>>> {
    Ok(manifest_runtime::typed_file_manifest(file_manifest)?.chunks)
}

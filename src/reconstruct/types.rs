use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tempfile::TempDir;

use crate::hash::ChunkHash;
use crate::manifest::CompressionUnit;
use crate::manifest::target::TargetManifest;
use crate::runtime::RuntimePlan;

#[derive(Debug, Clone, Default)]
pub struct ReconstructTimings {
    pub runtime_plan: Option<RuntimePlan>,
    pub bundle_read: Duration,
    pub bundle_read_compressed_bytes: u64,
    pub bundle_read_decompressed_bytes: u64,
    pub file_rebuild: Duration,
    pub file_rebuild_bytes: u64,
    pub file_verify: Duration,
    pub file_verify_bytes: u64,
    pub file_write: Duration,
    pub file_write_bytes: u64,
    pub total: Duration,
}

pub struct VerifiedSourceInstall<'a> {
    pub target_manifest: &'a TargetManifest,
    pub install_dir: &'a Path,
}

#[derive(Debug, Clone, Copy)]
pub struct ReconstructOptions {
    pub verify_chunks: bool,
    pub verify_files: bool,
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
pub(super) struct ChunkWriteTarget {
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
    pub compressed_bytes: u64,
    pub decompressed_bytes: u64,
}

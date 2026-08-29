use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::hash::ChunkHash;
use crate::manifest::{
    ArtifactStorageMode, BundleSizingProfile, ChunkSizingProfile, CompressionProfile, FileManifest,
    SnapshotTagSet, snapshot::SnapshotManifest,
};
use crate::runtime::{RuntimePlan, RuntimeResourceHints};

#[derive(Debug, Clone)]
pub struct BuildConfig<'a> {
    pub gutch_repo_id: String,
    pub snapshot_id: String,
    pub snapshot_tags: SnapshotTagSet,
    pub existing_chunk_locations: Option<&'a HashMap<String, ExistingChunkLocation>>,
    pub existing_bundle_ids: Option<&'a HashSet<String>>,
    pub previous_file_manifests: Option<&'a HashMap<String, Vec<FileManifest>>>,
    pub chunk_profile: ChunkSizingProfile,
    pub bundle_profile: BundleSizingProfile,
    pub compression_profile: CompressionProfile,
    pub storage_mode: ArtifactStorageMode,
    pub runtime_hints: RuntimeResourceHints,
    pub control: crate::GutchOperationControl,
}

impl<'a> BuildConfig<'a> {
    pub fn with_ids(
        gutch_repo_id: &str,
        snapshot_id: &str,
        snapshot_tags: &SnapshotTagSet,
    ) -> BuildConfig<'a> {
        BuildConfig {
            gutch_repo_id: gutch_repo_id.to_string(),
            snapshot_id: snapshot_id.to_string(),
            snapshot_tags: snapshot_tags.clone(),
            existing_chunk_locations: None,
            existing_bundle_ids: None,
            previous_file_manifests: None,
            chunk_profile: ChunkSizingProfile::default(),
            bundle_profile: BundleSizingProfile::default(),
            compression_profile: CompressionProfile::default(),
            storage_mode: ArtifactStorageMode::default(),
            runtime_hints: RuntimeResourceHints::default(),
            control: crate::GutchOperationControl::default(),
        }
    }

    pub fn with_existing_bundles(mut self, ids: &'a HashSet<String>) -> Self {
        self.existing_bundle_ids = Some(ids);
        self
    }

    pub fn with_existing_chunks(
        mut self,
        locations: &'a HashMap<String, ExistingChunkLocation>,
    ) -> Self {
        self.existing_chunk_locations = Some(locations);
        self
    }

    pub fn with_previous_manifests(
        mut self,
        manifests: &'a HashMap<String, Vec<FileManifest>>,
    ) -> Self {
        self.previous_file_manifests = Some(manifests);
        self
    }

    pub fn with_chunk_profile(mut self, profile: &ChunkSizingProfile) -> Self {
        self.chunk_profile = profile.clone();
        self
    }

    pub fn with_bundle_profile(mut self, profile: &BundleSizingProfile) -> Self {
        self.bundle_profile = profile.clone();
        self
    }

    pub fn with_compression_profile(mut self, profile: &CompressionProfile) -> Self {
        self.compression_profile = profile.clone();
        self
    }

    pub fn with_control(mut self, control: crate::GutchOperationControl) -> Self {
        self.control = control;
        self
    }
}

pub struct BuildResult {
    pub snapshot_manifest: SnapshotManifest,
    pub bundles: Vec<BuiltBundle>,
    pub loose_chunks: Vec<BuiltLooseChunk>,
}

#[derive(Debug, Clone)]
pub struct BuiltBundle {
    pub bundle_id: String,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct BuiltLooseChunk {
    pub chunk_hash: ChunkHash,
    pub data: Vec<u8>,
    pub raw_size: u64,
    pub compressed_size: u64,
}

#[derive(Debug, Clone)]
pub struct ExistingChunkLocation {
    pub bundle_id: String,
    pub raw_size: u64,
    pub compressed_size: u64,
    pub storage_mode: ArtifactStorageMode,
}

#[derive(Debug, Clone)]
pub enum ChunkPayload {
    InMemory {
        bytes: Arc<[u8]>,
    },
    TempFile {
        path: PathBuf,
        offset: u64,
        raw_size: u64,
    },
}

impl ChunkPayload {
    pub fn raw_size(&self) -> u64 {
        match self {
            ChunkPayload::InMemory { bytes } => bytes.len() as u64,
            ChunkPayload::TempFile { raw_size, .. } => *raw_size,
        }
    }

    pub fn load_bytes(&self) -> crate::Result<Vec<u8>> {
        self.with_bytes(|bytes| bytes.to_vec())
    }

    pub fn with_bytes<T>(&self, f: impl FnOnce(&[u8]) -> T) -> crate::Result<T> {
        match self {
            ChunkPayload::InMemory { bytes } => Ok(f(bytes.as_ref())),
            ChunkPayload::TempFile {
                path,
                offset,
                raw_size,
            } => {
                let mut file = std::fs::File::open(path)?;
                file.seek(SeekFrom::Start(*offset))?;
                let mut bytes = vec![0u8; *raw_size as usize];
                file.read_exact(&mut bytes)?;
                Ok(f(&bytes))
            }
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct BuildTimings {
    pub storage_mode: ArtifactStorageMode,
    pub scan_chunk_wall: Duration,
    pub logical_bytes: u64,
    pub file_hash: Duration,
    pub file_hash_bytes: u64,
    pub stream_chunk: Duration,
    pub stream_chunk_bytes: u64,
    pub chunk_hash_judgment: Duration,
    pub chunk_hash_judgment_bytes: u64,
    pub metadata: Duration,
    pub dedupe_plan_wall: Duration,
    pub dedupe_lookup: Duration,
    pub dedupe_lookup_bytes: u64,
    pub artifact_encode_wall: Duration,
    pub compression_input_bytes: u64,
    pub compression_output_bytes: u64,
    pub total_compressed_bytes: u64,
    pub bundle_compressed_sort: Duration,
    pub bundle_compressed_sort_bytes: u64,
    pub bundle_compressed_ratio_probe: Duration,
    pub bundle_compressed_ratio_probe_bytes: u64,
    pub bundle_compressed_payload_load: Duration,
    pub bundle_compressed_payload_load_bytes: u64,
    pub bundle_compressed_payload_assembly: Duration,
    pub bundle_compressed_payload_assembly_bytes: u64,
    pub bundle_compressed_stream_compression: Duration,
    pub bundle_compressed_stream_input_bytes: u64,
    pub bundle_compressed_stream_output_bytes: u64,
    pub bundle_compressed_serialization: Duration,
    pub bundle_compressed_serialization_bytes: u64,
    pub artifact_output_bytes: u64,
    pub manifest_finalize_wall: Duration,
    pub total: Duration,
    pub runtime_plan: Option<RuntimePlan>,
}

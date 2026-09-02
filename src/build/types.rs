use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use crate::hash::ChunkHash;
use crate::manifest::{
    ArtifactSizingPreset, ArtifactStorageMode, BundleSizingPreset, BundleSizingProfile,
    ChunkSizingProfile, CompressionPreset, CompressionProfile, FileManifest, SnapshotTagSet,
    snapshot::SnapshotManifest,
};
use crate::runtime::{RuntimePlan, RuntimeResourceHints};

#[derive(Debug, Clone)]
pub struct BuildConfig<'a> {
    pub(crate) snapshot_id: String,
    pub(crate) snapshot_tags: SnapshotTagSet,
    pub(crate) existing_chunk_locations: Option<&'a HashMap<String, ExistingChunkLocation>>,
    pub(crate) existing_bundle_ids: Option<&'a HashSet<String>>,
    pub(crate) previous_file_manifests: Option<&'a HashMap<String, Vec<FileManifest>>>,
    pub(crate) chunk_profile: ChunkSizingProfile,
    pub(crate) bundle_profile: BundleSizingProfile,
    pub(crate) compression_profile: CompressionProfile,
    pub(crate) storage_mode: ArtifactStorageMode,
    pub(crate) runtime_hints: RuntimeResourceHints,
    pub(crate) control: crate::GutchOperationControl,
}

impl<'a> BuildConfig<'a> {
    pub fn for_snapshot(snapshot_id: &str, snapshot_tags: &SnapshotTagSet) -> BuildConfig<'a> {
        BuildConfig {
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

    pub(crate) fn with_existing_bundles(mut self, ids: &'a HashSet<String>) -> Self {
        self.existing_bundle_ids = Some(ids);
        self
    }

    pub(crate) fn with_existing_chunks(
        mut self,
        locations: &'a HashMap<String, ExistingChunkLocation>,
    ) -> Self {
        self.existing_chunk_locations = Some(locations);
        self
    }

    pub(crate) fn with_previous_manifests(
        mut self,
        manifests: &'a HashMap<String, Vec<FileManifest>>,
    ) -> Self {
        self.previous_file_manifests = Some(manifests);
        self
    }

    pub fn with_chunk_profile(mut self, preset: ArtifactSizingPreset) -> Self {
        self.chunk_profile = ChunkSizingProfile::for_preset(preset);
        self
    }

    pub fn with_bundle_profile(mut self, preset: BundleSizingPreset) -> Self {
        self.bundle_profile = BundleSizingProfile::for_preset(preset);
        self
    }

    pub fn with_compression_profile(mut self, preset: CompressionPreset) -> Self {
        self.compression_profile = CompressionProfile::for_preset(preset);
        self
    }

    pub fn with_control(mut self, control: crate::GutchOperationControl) -> Self {
        self.control = control;
        self
    }

    pub fn with_storage_mode(mut self, storage_mode: ArtifactStorageMode) -> Self {
        self.storage_mode = storage_mode;
        self
    }

    pub fn with_runtime_hints(mut self, runtime_hints: RuntimeResourceHints) -> Self {
        self.runtime_hints = runtime_hints;
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
pub(crate) struct ExistingChunkLocation {
    pub bundle_id: String,
    pub compressed_size: u64,
    pub storage_mode: ArtifactStorageMode,
}

#[derive(Debug, Clone)]
pub(crate) enum ChunkPayload {
    InMemory { bytes: Arc<[u8]> },
}

impl ChunkPayload {
    pub(crate) fn raw_size(&self) -> u64 {
        match self {
            ChunkPayload::InMemory { bytes } => bytes.len() as u64,
        }
    }

    pub(crate) fn load_bytes(&self) -> crate::Result<Vec<u8>> {
        self.with_bytes(|bytes| bytes.to_vec())
    }

    pub(crate) fn with_bytes<T>(&self, f: impl FnOnce(&[u8]) -> T) -> crate::Result<T> {
        match self {
            ChunkPayload::InMemory { bytes } => Ok(f(bytes.as_ref())),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct BuildTimings {
    pub(crate) storage_mode: ArtifactStorageMode,
    pub(crate) scan_chunk_wall: Duration,
    pub(crate) logical_bytes: u64,
    pub(crate) file_hash: Duration,
    pub(crate) file_hash_bytes: u64,
    pub(crate) stream_chunk: Duration,
    pub(crate) stream_chunk_bytes: u64,
    pub(crate) chunk_hash_judgment: Duration,
    pub(crate) chunk_hash_judgment_bytes: u64,
    pub(crate) metadata: Duration,
    pub(crate) dedupe_plan_wall: Duration,
    pub(crate) dedupe_lookup: Duration,
    pub(crate) dedupe_lookup_bytes: u64,
    pub(crate) artifact_encode_wall: Duration,
    pub(crate) compression_input_bytes: u64,
    pub(crate) compression_output_bytes: u64,
    pub(crate) total_compressed_bytes: u64,
    pub(crate) bundle_compressed_sort: Duration,
    pub(crate) bundle_compressed_sort_bytes: u64,
    pub(crate) bundle_compressed_ratio_probe: Duration,
    pub(crate) bundle_compressed_ratio_probe_bytes: u64,
    pub(crate) bundle_compressed_payload_load: Duration,
    pub(crate) bundle_compressed_payload_load_bytes: u64,
    pub(crate) bundle_compressed_payload_assembly: Duration,
    pub(crate) bundle_compressed_payload_assembly_bytes: u64,
    pub(crate) bundle_compressed_stream_compression: Duration,
    pub(crate) bundle_compressed_stream_input_bytes: u64,
    pub(crate) bundle_compressed_stream_output_bytes: u64,
    pub(crate) bundle_compressed_serialization: Duration,
    pub(crate) bundle_compressed_serialization_bytes: u64,
    pub(crate) artifact_output_bytes: u64,
    pub(crate) manifest_finalize_wall: Duration,
    pub(crate) total: Duration,
    pub(crate) runtime_plan: Option<RuntimePlan>,
}

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::artifact::hash::ChunkHash;
use crate::manifest::ArtifactStorageMode;
use crate::manifest::BundleSizingPreset;
use crate::manifest::BundleSizingProfile;
use crate::manifest::ChunkSizingPreset;
use crate::manifest::ChunkSizingProfile;
use crate::manifest::CompressionKind;
use crate::manifest::CompressionProfile;
use crate::manifest::FileManifest;
use crate::manifest::SnapshotTagSet;
use crate::manifest::snapshot::SnapshotManifest;
use crate::runtime::RuntimePlan;
use crate::runtime::RuntimeResourceHints;

#[derive(Debug, Clone)]
pub struct BuildConfig<'a> {
    pub(crate) snapshot_tags: SnapshotTagSet,
    pub(crate) existing_chunk_locations: Option<&'a HashMap<String, ExistingChunkLocation>>,
    pub(crate) existing_bundle_ids: Option<&'a HashSet<String>>,
    pub(crate) previous_file_manifests: Option<&'a HashMap<String, Vec<FileManifest>>>,
    pub(crate) chunk_profile: ChunkSizingProfile,
    pub(crate) bundle_profile: BundleSizingProfile,
    pub(crate) compression_profile: CompressionProfile,
    pub(crate) storage_mode: ArtifactStorageMode,
    pub(crate) runtime_hints: RuntimeResourceHints,
    pub(crate) artifact_staging_root: Option<PathBuf>,
    pub(crate) control: crate::progress::GutchOperationControl,
}

impl<'a> BuildConfig<'a> {
    pub fn for_snapshot(snapshot_tags: &SnapshotTagSet) -> BuildConfig<'a> {
        BuildConfig {
            snapshot_tags: snapshot_tags.clone(),
            existing_chunk_locations: None,
            existing_bundle_ids: None,
            previous_file_manifests: None,
            chunk_profile: ChunkSizingProfile::default(),
            bundle_profile: BundleSizingProfile::default(),
            compression_profile: CompressionProfile::default(),
            storage_mode: ArtifactStorageMode::default(),
            runtime_hints: RuntimeResourceHints::default(),
            artifact_staging_root: None,
            control: crate::progress::GutchOperationControl::default(),
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

    pub fn with_chunk_profile(mut self, preset: ChunkSizingPreset) -> Self {
        self.chunk_profile =
            ChunkSizingProfile::for_preset(preset).with_chunker(self.chunk_profile.chunker);
        self
    }

    pub fn with_chunker(mut self, chunker: crate::manifest::Chunker) -> Self {
        self.chunk_profile = self.chunk_profile.with_chunker(chunker);
        self
    }

    pub fn with_bundle_profile(mut self, preset: BundleSizingPreset) -> Self {
        self.bundle_profile = BundleSizingProfile::for_preset(preset);
        self
    }

    pub fn with_compression(mut self, algorithm: CompressionKind, level: Option<i32>) -> Self {
        self.compression_profile = CompressionProfile::for_algorithm(algorithm, level);
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

    pub fn with_artifact_staging_root(mut self, path: impl Into<PathBuf>) -> Self {
        self.artifact_staging_root = Some(path.into());
        self
    }
}

pub struct BuildResult {
    pub snapshot_manifest: SnapshotManifest,
    pub bundles: Vec<BuiltBundle>,
    pub loose_chunks: Vec<BuiltLooseChunk>,
}

#[derive(Debug)]
pub(crate) struct StagedArtifactFile {
    path: PathBuf,
}

impl StagedArtifactFile {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub(crate) fn path(&self) -> &std::path::Path {
        &self.path
    }

    pub(crate) fn promote_to(&self, destination: &std::path::Path) -> crate::Result<()> {
        std::fs::rename(&self.path, destination)?;
        Ok(())
    }

    pub(crate) fn discard(&self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

impl Drop for StagedArtifactFile {
    fn drop(&mut self) {
        self.discard();
    }
}

#[derive(Debug)]
pub struct BuiltBundle {
    pub bundle_id: String,
    pub(crate) staged_file: StagedArtifactFile,
    pub(crate) size: u64,
    pub(crate) entries: Vec<crate::artifact::bundle::BundleTableEntry>,
}

impl BuiltBundle {
    pub(crate) fn discard_staged_file(&self) {
        let _ = std::fs::remove_file(self.staged_file.path());
    }
}

#[derive(Debug)]
pub struct BuiltLooseChunk {
    pub chunk_hash: ChunkHash,
    pub(crate) staged_file: StagedArtifactFile,
    pub raw_size: u64,
    pub compressed_size: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct ExistingChunkLocation {
    pub bundle_id: String,
    pub compressed_size: u64,
    pub storage_mode: ArtifactStorageMode,
}

/// A scanned source range, loaded only while encoding its artifact.
#[derive(Debug, Clone)]
pub(crate) struct ChunkPayload {
    pub(crate) source: Arc<PathBuf>,
    pub(crate) offset: u64,
    pub(crate) size: u64,
    pub(crate) hash: ChunkHash,
}

impl ChunkPayload {
    pub(crate) fn raw_size(&self) -> u64 {
        self.size
    }

    pub(crate) fn load_bytes(&self) -> crate::Result<Vec<u8>> {
        use std::io::Read;
        use std::io::Seek;
        use std::io::SeekFrom;
        let mut file = std::fs::File::open(self.source.as_ref())?;
        file.seek(SeekFrom::Start(self.offset))?;
        let mut bytes = vec![
            0;
            usize::try_from(self.size).map_err(|_| crate::Error::manifest(
                "chunk exceeds address space"
            ))?
        ];
        file.read_exact(&mut bytes)?;
        if ChunkHash::from_blake3(blake3::hash(&bytes)) != self.hash {
            return Err(crate::Error::manifest(format!(
                "source changed during capture: {}",
                self.source.display()
            )));
        }
        Ok(bytes)
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
    pub(crate) bundle_compressed_frame_compression: Duration,
    pub(crate) bundle_compressed_frame_input_bytes: u64,
    pub(crate) bundle_compressed_frame_output_bytes: u64,
    pub(crate) bundle_compressed_footer_serialization: Duration,
    pub(crate) bundle_compressed_footer_serialization_bytes: u64,
    pub(crate) artifact_output_bytes: u64,
    pub(crate) manifest_finalize_wall: Duration,
    pub(crate) total: Duration,
    pub(crate) runtime_plan: Option<RuntimePlan>,
}

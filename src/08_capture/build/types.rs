//! Build configuration, staged artifacts, chunk payloads and build results.

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::artifact::bundle::BundleTableEntry;
use crate::artifact::hash::ChunkHash;
use crate::artifact::profiles::ArtifactStorageMode;
use crate::artifact::profiles::BundleSizingPreset;
use crate::artifact::profiles::BundleSizingProfile;
use crate::artifact::profiles::ChunkSizingPreset;
use crate::artifact::profiles::ChunkSizingProfile;
use crate::artifact::profiles::Chunker;
use crate::artifact::profiles::CompressionKind;
use crate::artifact::profiles::CompressionProfile;
use crate::manifest::SnapshotTagSet;
use crate::manifest::snapshot::SnapshotManifest;
use crate::progress::OperationControl;
use crate::runtime::RuntimePlan;
use crate::runtime::RuntimeResourceHints;
use std::fs;
use std::fs::File;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct BuildConfig<'a> {
    pub(crate) snapshot_tags: SnapshotTagSet,
    pub(crate) available_chunk_sizes: Option<&'a HashMap<ChunkHash, u64>>,
    pub(crate) existing_bundle_ids: Option<&'a HashSet<String>>,
    pub(crate) chunk_profile: ChunkSizingProfile,
    pub(crate) bundle_profile: BundleSizingProfile,
    pub(crate) compression_profile: CompressionProfile,
    pub(crate) storage_mode: ArtifactStorageMode,
    pub(crate) runtime_hints: RuntimeResourceHints,
    pub(crate) artifact_staging_root: Option<PathBuf>,
    pub(crate) control: OperationControl,
}

impl<'a> BuildConfig<'a> {
    pub fn for_snapshot(snapshot_tags: &SnapshotTagSet) -> BuildConfig<'a> {
        BuildConfig {
            snapshot_tags: snapshot_tags.clone(),
            available_chunk_sizes: None,
            existing_bundle_ids: None,
            chunk_profile: ChunkSizingProfile::default(),
            bundle_profile: BundleSizingProfile::default(),
            compression_profile: CompressionProfile::default(),
            storage_mode: ArtifactStorageMode::default(),
            runtime_hints: RuntimeResourceHints::default(),
            artifact_staging_root: None,
            control: OperationControl::default(),
        }
    }

    pub(crate) fn with_existing_bundles(mut self, ids: &'a HashSet<String>) -> Self {
        self.existing_bundle_ids = Some(ids);
        self
    }

    pub(crate) fn with_existing_chunks(mut self, sizes: &'a HashMap<ChunkHash, u64>) -> Self {
        self.available_chunk_sizes = Some(sizes);
        self
    }

    pub fn with_chunk_profile(mut self, preset: ChunkSizingPreset) -> Self {
        self.chunk_profile =
            ChunkSizingProfile::for_preset(preset).with_chunker(self.chunk_profile.chunker);
        self
    }

    pub fn with_chunker(mut self, chunker: Chunker) -> Self {
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

    #[cfg(test)]
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Staging leaves bytes unsynced; flush them here so no artifact becomes visible before it is durable.
    pub(crate) fn promote_to(&self, destination: &Path) -> crate::Result<()> {
        // Windows flushes file buffers only through a handle opened for writing.
        fs::OpenOptions::new()
            .write(true)
            .open(&self.path)?
            .sync_all()?;
        fs::rename(&self.path, destination)?;
        Ok(())
    }

    /// Artifacts are content-addressed, so an existing destination already holds these bytes.
    /// Replacing it would fail on Windows while another reader has the file open.
    pub(crate) fn promote_unless_present(&self, destination: &Path) -> crate::Result<()> {
        if destination.exists() {
            self.discard();
            Ok(())
        } else {
            self.promote_to(destination)
        }
    }

    pub(crate) fn discard(&self) {
        let _ = fs::remove_file(&self.path);
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
    pub(crate) entries: Vec<BundleTableEntry>,
}

#[derive(Debug)]
pub struct BuiltLooseChunk {
    pub chunk_hash: ChunkHash,
    pub(crate) staged_file: StagedArtifactFile,
    pub raw_size: u64,
    pub compressed_size: u64,
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
        ChunkPayloadReader::default().load(self)
    }
}

/// Retain only the current source handle: bundle planning groups nearby chunks.
/// Each packaging job owns its reader, so there are no shared cursors or locks.
#[derive(Default)]
pub(crate) struct ChunkPayloadReader {
    current: Option<(Arc<PathBuf>, File)>,
    position: Option<u64>,
}

impl ChunkPayloadReader {
    pub(crate) fn load(&mut self, payload: &ChunkPayload) -> crate::Result<Vec<u8>> {
        use std::io::{Read, Seek, SeekFrom};
        if self
            .current
            .as_ref()
            .is_none_or(|(path, _)| path != &payload.source)
        {
            // Close the previous handle before opening the next source.
            self.current = None;
            self.position = None;
            let file = File::open(payload.source.as_ref())?;
            self.current = Some((payload.source.clone(), file));
            self.position = Some(0);
        }
        let (_, file) = self.current.as_mut().expect("source handle was opened");
        if self.position != Some(payload.offset) {
            self.position = None;
            file.seek(SeekFrom::Start(payload.offset))?;
        }
        let mut bytes = vec![
            0;
            usize::try_from(payload.size).map_err(|_| crate::Error::capture(
                "chunk exceeds address space"
            ))?
        ];
        // A failed read may advance the cursor partially. Invalidate it first
        // so any later load seeks explicitly; successful reads establish it anew.
        self.position = None;
        file.read_exact(&mut bytes)?;
        self.position = payload.offset.checked_add(payload.size);
        // Reusing a handle never skips validation against the scan-time hash.
        if ChunkHash::of(&bytes) != payload.hash {
            return Err(crate::Error::SourceChanged {
                path: Some(payload.source.as_ref().clone()),
            });
        }
        Ok(bytes)
    }
}

#[derive(Debug, Clone, Default)]
pub struct BuildTimings {
    pub(crate) storage_mode: ArtifactStorageMode,
    pub(crate) prepare_wall: Duration,
    pub(crate) scan_chunk_wall: Duration,
    pub(crate) logical_bytes: u64,
    pub(crate) file_hash: Duration,
    pub(crate) file_hash_bytes: u64,
    pub(crate) stream_chunk: Duration,
    pub(crate) stream_chunk_bytes: u64,
    pub(crate) chunk_hash_judgment: Duration,
    pub(crate) chunk_hash_judgment_bytes: u64,
    pub(crate) dedupe_plan_wall: Duration,
    pub(crate) dedupe_lookup: Duration,
    pub(crate) artifact_encode_wall: Duration,
    pub(crate) compression_input_bytes: u64,
    pub(crate) compression_output_bytes: u64,
    pub(crate) total_compressed_bytes: u64,
    pub(crate) bundle_compressed_sort: Duration,
    pub(crate) bundle_compressed_ratio_probe: Duration,
    pub(crate) bundle_compressed_ratio_probe_bytes: u64,
    pub(crate) artifact_payload_load: Duration,
    pub(crate) artifact_payload_load_bytes: u64,
    pub(crate) bundle_compressed_frame_compression: Duration,
    pub(crate) bundle_compressed_frame_input_bytes: u64,
    pub(crate) bundle_compressed_frame_output_bytes: u64,
    pub(crate) bundle_compressed_footer_serialization: Duration,
    pub(crate) bundle_compressed_footer_serialization_bytes: u64,
    pub(crate) artifact_output_bytes: u64,
    pub(crate) manifest_finalize_wall: Duration,
    pub(crate) publish_wall: Duration,
    pub(crate) total: Duration,
    pub(crate) runtime_plan: Option<RuntimePlan>,
}

#[cfg(test)]
mod tests {
    #[test]
    fn promotion_keeps_an_existing_artifact_and_discards_the_staged_copy() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("artifact");
        std::fs::write(&destination, b"existing").unwrap();
        let staged_path = temp.path().join("staged");
        std::fs::write(&staged_path, b"staged").unwrap();

        super::StagedArtifactFile::new(staged_path.clone())
            .promote_unless_present(&destination)
            .unwrap();

        assert_eq!(std::fs::read(&destination).unwrap(), b"existing");
        assert!(!staged_path.exists());
    }

    #[test]
    fn promotion_moves_the_staged_file_into_a_new_destination() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("artifact");
        let staged_path = temp.path().join("staged");
        std::fs::write(&staged_path, b"staged").unwrap();

        super::StagedArtifactFile::new(staged_path.clone())
            .promote_unless_present(&destination)
            .unwrap();

        assert_eq!(std::fs::read(&destination).unwrap(), b"staged");
        assert!(!staged_path.exists());
    }
}

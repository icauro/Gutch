//! The reconstruction plan the executor runs: per-artifact and per-source jobs and their targets.

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use crate::artifact::compression::decode_working_memory_bytes;
use crate::artifact::hash::ChunkHash;
use crate::artifact::reference::ArtifactRef;
use crate::manifest::snapshot::SnapshotManifest;
use crate::progress::OperationControl;
use crate::runtime::RuntimeResourceHints;
use crate::transfer::download::PendingArtifactDownload;
use crate::transfer::http::DownloadClient;
use crate::transfer::progress::FetchTransferProgress;
use crate::transfer::types::TransferConcurrencyPolicy;
use std::iter;
use std::ops::Range;

pub(crate) struct ReconstructPipelinePlan {
    pub(crate) files: Vec<ReconstructPipelineFile>,
    pub(crate) jobs: Vec<ReconstructJob>,
    pub(crate) missing_artifact_to_file_indices: HashMap<ArtifactRef, Vec<usize>>,
    pub(crate) missing_artifact_counts: Vec<usize>,
    pub required_bundle_count: usize,
    pub required_artifacts: HashSet<ArtifactRef>,
    pub required_artifact_count: usize,
    pub(crate) required_chunk_count: usize,
    pub(crate) reused_chunk_count: usize,
}

pub(crate) struct ReconstructPipelineApplyRequest<'a> {
    pub publication: super::publish::Publication<'a>,
    pub repo_root: &'a Path,
    pub snapshot_manifest: &'a SnapshotManifest,
    pub destination_dir: &'a Path,
    pub staging_dir: Option<&'a Path>,
    pub reuse_dir: Option<PathBuf>,
    pub runtime_hints: &'a RuntimeResourceHints,
    pub existing_artifacts: &'a HashSet<ArtifactRef>,
    pub pending_artifact_downloads: &'a [PendingArtifactDownload],
    pub transfer_policy: TransferConcurrencyPolicy,
    pub control: &'a OperationControl,
    pub download_client: Option<&'a DownloadClient>,
    pub transfer_progress: Option<&'a Arc<FetchTransferProgress>>,
}

#[derive(Clone)]
pub(crate) struct ReconstructPipelineFile {
    pub(crate) path: String,
    pub(crate) size: u64,
    pub(crate) full_file_hash: ChunkHash,
}

#[derive(Clone)]
pub(crate) enum ReconstructJob {
    Artifact(ReconstructArtifactJob),
    SourceChunk(ReconstructSourceChunkJob),
    CopySourceFile {
        file_index: usize,
        source_path: PathBuf,
    },
}

#[derive(Clone)]
pub(crate) struct ReconstructArtifactJob {
    pub(crate) artifact: ArtifactRef,
    pub(crate) artifact_path: PathBuf,
    /// Complete encoded length declared by the index, including artifact framing.
    pub(crate) encoded_size: u32,
    /// Raw size of every chunk the index places in the artifact, required or not:
    /// decoding streams through all of them.
    pub(crate) decoded_size: u64,
    pub(crate) chunks: Vec<ReconstructArtifactChunk>,
    pub(crate) targets: Vec<ReconstructTarget>,
}

#[derive(Clone)]
pub(crate) struct ReconstructArtifactChunk {
    pub(crate) chunk_hash: ChunkHash,
    pub(crate) raw_size: u32,
    pub(crate) target_range: Range<usize>,
}

#[derive(Clone)]
pub(crate) struct ReconstructSourceChunkJob {
    pub(crate) chunk_hash: ChunkHash,
    pub(crate) raw_size: u32,
    pub(crate) source_path: Arc<PathBuf>,
    pub(crate) source_offset: u64,
    // Every source job starts with one target; keep it inline to avoid a per-chunk allocation.
    pub(crate) first_target: ReconstructTarget,
    pub(crate) additional_targets: Vec<ReconstructTarget>,
}

impl ReconstructSourceChunkJob {
    pub(crate) fn targets(&self) -> impl Iterator<Item = &ReconstructTarget> {
        iter::once(&self.first_target).chain(self.additional_targets.iter())
    }
}

#[derive(Clone)]
pub(crate) struct ReconstructTarget {
    pub(crate) file_index: usize,
    pub(crate) file_offset: u64,
}

pub(crate) struct SourceChunkLocation {
    pub(crate) path: Arc<PathBuf>,
    pub(crate) offset: u64,
    pub(crate) raw_size: u64,
}

impl ReconstructJob {
    pub(crate) fn memory_reservation_bytes(&self) -> u64 {
        const SOURCE_COPY_BUFFER_BYTES: u64 = 1024 * 1024;
        match self {
            Self::Artifact(job) => {
                decode_working_memory_bytes(job.decoded_size)
                    + super::handles::RECONSTRUCT_OUTPUT_BUFFER_BYTES as u64
            }
            Self::SourceChunk(chunk) => u64::from(chunk.raw_size) + SOURCE_COPY_BUFFER_BYTES,
            Self::CopySourceFile { .. } => SOURCE_COPY_BUFFER_BYTES,
        }
    }
}

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use crate::manifest::snapshot::SnapshotManifest;
use crate::progress::GutchOperationControl;
use crate::runtime::RuntimeResourceHints;
use crate::transfer::download::FetchTransferProgress;
use crate::transfer::download::PendingBundleDownload;

pub(crate) struct ReconstructPipelinePlan {
    pub(crate) files: Vec<ReconstructPipelineFile>,
    pub(crate) jobs: Vec<ReconstructJob>,
    pub(crate) missing_artifact_to_file_indices:
        HashMap<crate::repo::artifacts::ArtifactRef, Vec<usize>>,
    pub(crate) missing_artifact_counts: Vec<usize>,
    pub required_bundle_count: usize,
    pub required_artifacts: HashSet<crate::repo::artifacts::ArtifactRef>,
    pub required_artifact_count: usize,
    pub(crate) required_chunk_count: usize,
    pub(crate) reused_source_chunk_count: usize,
}

pub(crate) struct ReconstructPipelineApplyRequest<'a> {
    pub publication: super::publish::Publication<'a>,
    pub repo_root: &'a Path,
    pub compression_profile: crate::manifest::CompressionProfile,
    pub snapshot_manifest: &'a SnapshotManifest,
    pub destination_dir: &'a Path,
    pub staging_dir: Option<&'a Path>,
    pub source_destination_dir: Option<PathBuf>,
    pub runtime_hints: &'a RuntimeResourceHints,
    pub existing_artifacts: &'a HashSet<crate::repo::artifacts::ArtifactRef>,
    pub pending_bundle_downloads: &'a [PendingBundleDownload],
    pub transfer_policy: crate::transfer::types::TransferConcurrencyPolicy,
    pub control: &'a GutchOperationControl,
    pub download_client: Option<&'a reqwest::blocking::Client>,
    pub transfer_progress: Option<&'a Arc<FetchTransferProgress>>,
}

#[derive(Clone)]
pub(crate) struct ReconstructPipelineFile {
    pub(crate) path: String,
    pub(crate) size: u64,
    pub(crate) full_file_hash: crate::artifact::hash::ChunkHash,
    pub(crate) trace_metadata: ReconstructFileTraceMetadata,
    #[cfg(test)]
    pub(crate) reconstruction: ReconstructPipelineReconstruction,
}

#[derive(Clone, Copy, Default)]
pub(crate) struct ReconstructFileTraceMetadata {
    pub(crate) required_distinct_chunks: usize,
    pub(crate) required_distinct_artifacts: usize,
    pub(crate) required_distinct_bundles: usize,
    pub(crate) required_distinct_loose_chunks: usize,
}

#[derive(Clone)]
pub(crate) enum ReconstructPipelineReconstruction {
    #[cfg(test)]
    CopyFromSource,
    ReconstructFromParts {
        chunks: Vec<ReconstructPipelineChunkInput>,
    },
}

#[derive(Clone)]
pub(crate) enum ReconstructPipelineChunkInput {
    Source { chunk_hash: String },
    Artifact(ReconstructPipelineChunk),
}

#[derive(Clone)]
pub(crate) struct ReconstructPipelineChunk {
    pub(crate) chunk_hash: String,
    pub(crate) artifact: crate::repo::artifacts::ArtifactRef,
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
    pub(crate) artifact: crate::repo::artifacts::ArtifactRef,
    pub(crate) artifact_path: PathBuf,
    pub(crate) chunks: Vec<ReconstructArtifactChunk>,
    pub(crate) targets: Vec<ReconstructTarget>,
}

#[derive(Clone)]
pub(crate) struct ReconstructArtifactChunk {
    pub(crate) chunk_hash: crate::artifact::hash::ChunkHash,
    pub(crate) raw_size: u32,
    pub(crate) compressed_size: u32,
    pub(crate) targets: std::ops::Range<usize>,
}

#[derive(Clone)]
pub(crate) struct ReconstructSourceChunkJob {
    pub(crate) chunk_hash: String,
    pub(crate) raw_size: u32,
    pub(crate) source_path: PathBuf,
    pub(crate) source_offset: u64,
    pub(crate) targets: Vec<ReconstructTarget>,
}

#[derive(Clone)]
pub(crate) struct ReconstructTarget {
    pub(crate) file_index: usize,
    pub(crate) file_offset: u64,
}

pub(crate) struct SourceChunkLocation {
    pub(crate) path: PathBuf,
    pub(crate) offset: u64,
    pub(crate) raw_size: u64,
}

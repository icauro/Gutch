use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use rayon::prelude::*;

use super::transfer::FetchTransferProgress;
use super::transfer::PendingBundleDownload;
use crate::manifest::snapshot::SnapshotManifest;
use crate::progress::GutchOperationControl;
use crate::runtime::RuntimeResourceHints;

pub(super) struct ReconstructPipelinePlan {
    pub(super) files: Vec<ReconstructPipelineFile>,
    pub(super) jobs: Vec<ReconstructJob>,
    pub(super) missing_artifact_to_file_indices:
        HashMap<crate::artifact::store::ArtifactRef, Vec<usize>>,
    pub(super) missing_artifact_counts: Vec<usize>,
    pub required_bundle_count: usize,
    pub required_artifacts: HashSet<crate::artifact::store::ArtifactRef>,
    pub required_artifact_count: usize,
    pub(super) required_chunk_count: usize,
    pub(super) reused_source_chunk_count: usize,
}

pub(super) struct ReconstructPipelineApplyRequest<'a> {
    pub repo_root: &'a Path,
    pub compression_profile: crate::manifest::CompressionProfile,
    pub snapshot_manifest: &'a SnapshotManifest,
    pub destination_dir: &'a Path,
    pub staging_dir: Option<&'a Path>,
    pub source_destination_dir: Option<PathBuf>,
    pub runtime_hints: &'a RuntimeResourceHints,
    pub existing_artifacts: &'a HashSet<crate::artifact::store::ArtifactRef>,
    pub pending_bundle_downloads: &'a [PendingBundleDownload],
    pub transfer_policy: super::types::TransferConcurrencyPolicy,
    pub control: &'a GutchOperationControl,
    pub download_client: Option<&'a reqwest::blocking::Client>,
    pub transfer_progress: Option<&'a Arc<FetchTransferProgress>>,
}

#[derive(Clone)]
pub(super) struct ReconstructPipelineFile {
    pub(super) path: String,
    pub(super) size: u64,
    pub(super) full_file_hash: crate::artifact::hash::ChunkHash,
    pub(super) trace_metadata: ReconstructFileTraceMetadata,
    #[cfg(test)]
    pub(super) reconstruction: ReconstructPipelineReconstruction,
}

#[derive(Clone, Copy, Default)]
pub(super) struct ReconstructFileTraceMetadata {
    pub(super) required_distinct_chunks: usize,
    pub(super) required_distinct_artifacts: usize,
    pub(super) required_distinct_bundles: usize,
    pub(super) required_distinct_loose_chunks: usize,
}

#[derive(Clone)]
pub(super) enum ReconstructPipelineReconstruction {
    CopyFromSource,
    ReconstructFromParts {
        chunks: Vec<ReconstructPipelineChunkInput>,
    },
}

#[derive(Clone)]
pub(super) enum ReconstructPipelineChunkInput {
    Source { chunk_hash: String },
    Artifact(ReconstructPipelineChunk),
}

#[derive(Clone)]
pub(super) struct ReconstructPipelineChunk {
    pub(super) chunk_hash: String,
    pub(super) artifact: crate::artifact::store::ArtifactRef,
}

#[derive(Clone)]
pub(super) enum ReconstructJob {
    Artifact(ReconstructArtifactJob),
    SourceChunk(ReconstructSourceChunkJob),
    CopySourceFile {
        file_index: usize,
        source_path: PathBuf,
    },
}

#[derive(Clone)]
pub(super) struct ReconstructArtifactJob {
    pub(super) artifact: crate::artifact::store::ArtifactRef,
    pub(super) artifact_path: PathBuf,
    pub(super) chunks: Vec<ReconstructArtifactChunk>,
    pub(super) targets: Vec<ReconstructTarget>,
}

#[derive(Clone)]
pub(super) struct ReconstructArtifactChunk {
    pub(super) chunk_hash: crate::artifact::hash::ChunkHash,
    pub(super) raw_size: u32,
    pub(super) compressed_size: u32,
    pub(super) targets: std::ops::Range<usize>,
}

#[derive(Clone)]
pub(super) struct ReconstructSourceChunkJob {
    pub(super) chunk_hash: String,
    pub(super) raw_size: u32,
    pub(super) source_path: PathBuf,
    pub(super) source_offset: u64,
    pub(super) targets: Vec<ReconstructTarget>,
}

#[derive(Clone)]
pub(super) struct ReconstructTarget {
    pub(super) file_index: usize,
    pub(super) file_offset: u64,
}

pub(super) struct SourceChunkLocation {
    pub(super) path: PathBuf,
    pub(super) offset: u64,
    pub(super) raw_size: u64,
}

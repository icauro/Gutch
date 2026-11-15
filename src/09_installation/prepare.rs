//! Loads the snapshot and verifies reusable sources before planning.

use std::path::Path;

use crate::Result;
use crate::artifact::reference::ArtifactRef;
use crate::filesystem::constraints::validate_paths;
use crate::installation::contracts::SnapshotReconstructSource;
use crate::installation::execute::jobs::ReconstructPipelinePlan;
use crate::installation::plan::build_reconstruct_pipeline_plan;
use crate::installation::source::VerifiedSource;
use crate::installation::source::resolve_verified_source;
use crate::manifest::snapshot::SnapshotManifest;
use crate::progress::OperationControl;
use crate::repo::index::ChunkIndex;
use crate::repo::paths::RepoPaths;
use crate::repo::paths::load_gutch_repo_manifest;
use crate::repo::snapshot_manifests::load_snapshot_manifest;
use crate::runtime::RuntimeResourceHints;
use std::collections::HashSet;
use std::path::PathBuf;

/// A snapshot already present on disk whose unchanged files reconstruction may reuse.
pub(super) struct LoadedSnapshotSource {
    snapshot_manifest: SnapshotManifest,
    directory: PathBuf,
}

impl LoadedSnapshotSource {
    /// Loads the source snapshot when one is named; its files default to the destination.
    pub(super) fn load(
        repo_paths: &RepoPaths,
        source_snapshot_id: Option<&str>,
        source_dir: Option<&Path>,
        destination_dir: &Path,
    ) -> Result<Option<Self>> {
        let Some(source_snapshot_id) = source_snapshot_id else {
            return Ok(None);
        };
        Ok(Some(Self {
            snapshot_manifest: load_snapshot_manifest(repo_paths, source_snapshot_id)?,
            directory: source_dir.unwrap_or(destination_dir).to_path_buf(),
        }))
    }

    pub(super) fn as_source(&self) -> SnapshotReconstructSource<'_> {
        SnapshotReconstructSource {
            snapshot_manifest: &self.snapshot_manifest,
            destination_dir: &self.directory,
        }
    }
}

pub(super) struct LoadedRepoSnapshot {
    pub repo_paths: RepoPaths,
    pub snapshot_manifest: SnapshotManifest,
}

pub(super) fn load_repo_snapshot_manifest(
    repo_root: &Path,
    snapshot_id: &str,
) -> Result<LoadedRepoSnapshot> {
    let (repo_paths, repo_manifest) = load_gutch_repo_manifest(repo_root)?;
    let snapshot_manifest = load_snapshot_manifest(&repo_paths, snapshot_id)?;
    validate_paths(
        repo_manifest
            .constraints
            .as_ref()
            .map_or(&[], |constraints| constraints.targets.as_slice()),
        snapshot_manifest.entries.iter().map(|entry| entry.path()),
        "installation",
        Some(snapshot_id),
    )?;
    Ok(LoadedRepoSnapshot {
        repo_paths,
        snapshot_manifest,
    })
}

/// Shared preparation after each caller has acquired its repository and
/// destination guards. Remote acquisition/publication remain with transfer.
pub(crate) struct ReconstructInputs<'a> {
    pub snapshot_manifest: &'a SnapshotManifest,
    pub chunk_index: &'a ChunkIndex,
    pub repo_paths: &'a RepoPaths,
    pub existing_artifacts: &'a HashSet<ArtifactRef>,
    pub source: Option<SnapshotReconstructSource<'a>>,
    pub runtime_hints: &'a RuntimeResourceHints,
    pub control: &'a OperationControl,
}

pub(crate) struct PreparedReconstruction {
    pub plan: ReconstructPipelinePlan,
    pub source_directory: Option<PathBuf>,
}

pub(crate) fn prepare_reconstruction(
    inputs: ReconstructInputs<'_>,
) -> Result<PreparedReconstruction> {
    let source_directory = inputs
        .source
        .as_ref()
        .map(|source| source.destination_dir.to_path_buf());
    let verified_source =
        resolve_verified_source(inputs.source, inputs.runtime_hints, inputs.control)?;
    let _plan_span = inputs
        .control
        .profile_span("installation.reconstruct.prepare.plan");
    let plan = build_reconstruct_pipeline_plan(
        inputs.snapshot_manifest,
        inputs.chunk_index,
        inputs.repo_paths,
        inputs.existing_artifacts,
        verified_source.as_ref().map(VerifiedSource::source),
    )?;
    Ok(PreparedReconstruction {
        plan,
        source_directory,
    })
}

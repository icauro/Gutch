use std::path::Path;

use crate::Result;
use crate::manifest::snapshot::SnapshotManifest;
use crate::repo::paths::GutchRepoPaths;

pub(super) struct LoadedRepoSnapshot {
    pub repo_paths: GutchRepoPaths,
    pub repo_manifest: crate::manifest::gutch_repo::GutchRepoManifest,
    pub snapshot_manifest: SnapshotManifest,
}

pub(super) fn load_repo_snapshot_manifest(
    repo_root: &Path,
    snapshot_id: &str,
) -> Result<LoadedRepoSnapshot> {
    let (repo_paths, repo_manifest) = crate::repo::paths::load_gutch_repo_manifest(repo_root)?;
    let snapshot_manifest =
        crate::repo::snapshot_manifests::load_snapshot_manifest(&repo_paths, snapshot_id)?;
    crate::filesystem::constraints::validate_paths(
        &repo_manifest.constraints.clone().unwrap_or_default(),
        snapshot_manifest.entries.iter().map(|entry| entry.path()),
        "installation",
        Some(snapshot_id),
    )?;
    Ok(LoadedRepoSnapshot {
        repo_paths,
        repo_manifest,
        snapshot_manifest,
    })
}

/// Shared preparation after each caller has acquired its repository and
/// destination guards. Remote acquisition/publication remain with transfer.
pub(crate) struct ReconstructionInputs<'a> {
    pub snapshot_manifest: &'a SnapshotManifest,
    pub chunk_index: &'a crate::repo::index::ChunkIndex,
    pub repo_paths: &'a GutchRepoPaths,
    pub existing_artifacts: &'a std::collections::HashSet<crate::repo::artifacts::ArtifactRef>,
    pub source: Option<super::types::SnapshotReconstructSource<'a>>,
    pub runtime_hints: &'a crate::runtime::RuntimeResourceHints,
    pub control: &'a crate::progress::GutchOperationControl,
}

pub(crate) struct PreparedReconstruction {
    pub plan: super::jobs::ReconstructPipelinePlan,
    pub source_directory: Option<std::path::PathBuf>,
}

pub(crate) fn prepare_reconstruction(
    inputs: ReconstructionInputs<'_>,
) -> Result<PreparedReconstruction> {
    let source_directory = inputs
        .source
        .as_ref()
        .map(|source| source.destination_dir.to_path_buf());
    let verified_source =
        super::apply::resolve_verified_source(inputs.source, inputs.runtime_hints, inputs.control)?;
    let _plan_span = inputs
        .control
        .profile_span("installation.reconstruct.prepare.plan");
    let plan = super::plan::build_reconstruct_pipeline_plan(
        inputs.snapshot_manifest,
        inputs.chunk_index,
        inputs.repo_paths,
        inputs.existing_artifacts,
        verified_source
            .as_ref()
            .map(super::apply::VerifiedSource::source),
    )?;
    Ok(PreparedReconstruction {
        plan,
        source_directory,
    })
}

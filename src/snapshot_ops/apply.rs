use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::manifest::snapshot::SnapshotManifest;
use crate::runtime::RuntimeResourceHints;

use super::types::{
    SnapshotReconstructFromRepoRequest, SnapshotReconstructFromRepoResult,
    SnapshotReconstructResult, SnapshotReconstructSource, SnapshotVerifyFromRepoRequest,
    SnapshotVerifyFromRepoResult,
};
use crate::verify::{VerifyRequest, classify_directory};
use crate::{Error, Result};

pub(super) fn resolve_verified_source<'a>(
    source: Option<SnapshotReconstructSource<'a>>,
    runtime_hints: &RuntimeResourceHints,
    control: &crate::GutchOperationControl,
) -> Result<Option<SnapshotReconstructSource<'a>>> {
    control.checkpoint()?;
    match source {
        Some(source) => {
            let (source_classification, _) = classify_directory(VerifyRequest {
                snapshot_manifest: source.snapshot_manifest,
                dir: source.destination_dir,
                mode: crate::VerifyMode::Fast,
                runtime_hints,
                control: control.clone(),
            })?;
            if !source_classification.missing_files.is_empty()
                || !source_classification.corrupt_files.is_empty()
            {
                let source_errors = source_classification
                    .missing_files
                    .iter()
                    .chain(source_classification.corrupt_files.iter())
                    .map(|issue| issue.detail.as_str())
                    .collect::<Vec<_>>()
                    .join("; ");
                return Err(Error::SourceSnapshotNotVerified(source_errors));
            }

            Ok(Some(SnapshotReconstructSource {
                snapshot_manifest: source.snapshot_manifest,
                destination_dir: source.destination_dir,
            }))
        }
        None => Ok(None),
    }
}

pub(crate) fn default_snapshot_staging_dir(destination_dir: &Path, snapshot_id: &str) -> PathBuf {
    let destination_name = destination_dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("destination");
    destination_dir.with_file_name(format!(
        ".gutch-staging-{}-{}",
        destination_name, snapshot_id
    ))
}

pub fn verify_installed_snapshot(
    request: SnapshotVerifyFromRepoRequest,
) -> Result<SnapshotVerifyFromRepoResult> {
    request.control.checkpoint()?;
    let (repo_paths, snapshot_manifest) = super::snapshot_repo::load_repo_snapshot_manifest(
        &request.repo_path,
        &request.snapshot_id,
    )?;
    let (classification, verify_timings) = classify_directory(VerifyRequest {
        snapshot_manifest: &snapshot_manifest,
        dir: &request.destination_dir,
        mode: request.verify_mode,
        runtime_hints: &request.runtime_hints,
        control: request.control,
    })?;

    Ok(SnapshotVerifyFromRepoResult {
        repo_root: repo_paths.repo_root,
        snapshot_manifest,
        classification,
        verify_timings,
    })
}

pub fn reconstruct_snapshot_from_repo(
    request: SnapshotReconstructFromRepoRequest,
) -> Result<SnapshotReconstructFromRepoResult> {
    request.control.checkpoint()?;
    let (repo_paths, snapshot_manifest) = super::snapshot_repo::load_repo_snapshot_manifest(
        &request.repo_path,
        &request.snapshot_id,
    )?;
    let (_, repo_manifest) = crate::load_gutch_repo_manifest(&repo_paths.repo_root)?;
    let chunk_index = crate::load_chunk_index(&repo_paths)?;
    let required_artifacts = crate::required_artifacts(&snapshot_manifest, &chunk_index)?;
    let mut existing_artifacts = HashSet::new();
    for artifact in &required_artifacts {
        if artifact.local_path(&repo_paths)?.is_file() {
            existing_artifacts.insert(artifact.clone());
        }
    }

    let source_manifest = match request.source_snapshot_id.as_ref() {
        Some(source_snapshot_id) => Some(crate::load_snapshot_manifest(
            &repo_paths,
            source_snapshot_id,
        )?),
        None => None,
    };
    let source_destination_dir = source_manifest.as_ref().map(|_| {
        request
            .source_dir
            .clone()
            .unwrap_or_else(|| request.destination_dir.clone())
    });
    let source = source_manifest
        .as_ref()
        .zip(source_destination_dir.as_ref())
        .map(
            |(source_manifest, source_destination_dir)| SnapshotReconstructSource {
                snapshot_manifest: source_manifest,
                destination_dir: source_destination_dir,
            },
        );
    let verified_source =
        resolve_verified_source(source, &request.runtime_hints, &request.control)?;
    let pipeline_plan = super::pipeline::build_reconstruct_pipeline_plan(
        &snapshot_manifest,
        &chunk_index,
        &repo_paths,
        &existing_artifacts,
        verified_source,
    )?;
    super::pipeline::apply_reconstruct_pipeline(
        super::pipeline::ReconstructPipelineApplyRequest {
            repo_root: &repo_paths.repo_root,
            compression_profile: repo_manifest.compression_profile(),
            snapshot_manifest: &snapshot_manifest,
            destination_dir: &request.destination_dir,
            staging_dir: request.staging_dir.as_deref(),
            source_destination_dir,
            runtime_hints: &request.runtime_hints,
            existing_artifacts: &existing_artifacts,
            pending_bundle_downloads: &[],
            control: &request.control,
            download_client: None,
            transfer_progress: None,
        },
        pipeline_plan,
    )
}

pub(super) struct BuildSnapshotReconstructResultRequest<'a> {
    pub snapshot_manifest: &'a SnapshotManifest,
    pub destination_dir: &'a Path,
    pub staging_dir: &'a Path,
    pub reused_source_chunk_count: usize,
    pub required_chunk_count: usize,
    pub required_bundle_count: usize,
    pub required_artifact_count: usize,
    pub reconstruct_timings: super::types::ReconstructTimings,
    pub verify_timings: crate::verify::VerifyTimings,
}

pub(super) fn build_snapshot_reconstruct_result(
    request: BuildSnapshotReconstructResultRequest<'_>,
) -> SnapshotReconstructResult {
    SnapshotReconstructResult {
        snapshot_id: request.snapshot_manifest.snapshot_id.clone(),
        destination_dir: request.destination_dir.to_path_buf(),
        staging_dir: request.staging_dir.to_path_buf(),
        reused_source_chunk_count: request.reused_source_chunk_count,
        required_chunk_count: request.required_chunk_count,
        required_bundle_count: request.required_bundle_count,
        required_artifact_count: request.required_artifact_count,
        reconstruct_timings: request.reconstruct_timings,
        verify_timings: request.verify_timings,
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

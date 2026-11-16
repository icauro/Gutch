//! Combined download and reconstruction, preserving one repository lock and pipeline.
use crate::Result;
use crate::installation::contracts::InstallationReconstructResult;
use crate::installation::execute::apply_reconstruct_pipeline;
use crate::installation::execute::jobs::ReconstructPipelineApplyRequest;
use crate::installation::execute::publish::Publication;
use crate::installation::prepare::{
    LoadedSnapshotSource, ReconstructInputs, prepare_reconstruction,
};
use crate::installation::staging::prepare_installation_destination;
use crate::repo::coordination::write_with_control;
use crate::runtime::RuntimeResourceHints;
use crate::sync::fetch::{
    build_fetch_result, prepare_snapshot_fetch, publish_fetched_snapshot,
    select_pending_artifact_downloads,
};
use crate::sync::fetch_types::{SnapshotFetchRequest, SnapshotFetchResult};
use crate::transfer::http::build_download_client;
use crate::transfer::progress::FetchTransferProgress;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct InstallationFetchAndReconstructRequest {
    pub fetch_request: SnapshotFetchRequest,
    pub destination_dir: PathBuf,
    pub staging_dir: Option<PathBuf>,
    pub reuse_snapshot_id: Option<String>,
    pub reuse_from: Option<PathBuf>,
    pub runtime_hints: RuntimeResourceHints,
}

#[derive(Debug, Clone)]
pub struct InstallationFetchAndReconstructResult {
    pub fetch_result: SnapshotFetchResult,
    pub reconstruct_result: InstallationReconstructResult,
}

pub fn fetch_and_reconstruct_installation(
    request: &InstallationFetchAndReconstructRequest,
) -> Result<InstallationFetchAndReconstructResult> {
    request.fetch_request.control.checkpoint()?;
    fs::create_dir_all(&request.fetch_request.repo_root)?;
    write_with_control(
        &request.fetch_request.repo_root,
        &request.fetch_request.control.clone(),
        || {
            request.fetch_request.control.checkpoint()?;
            let _operation_span = request
                .fetch_request
                .control
                .profile_span("transfer.fetch_reconstruct");
            let progress = request.fetch_request.control.progress().clone();
            let download_client = build_download_client()?;
            let transfer_progress = Arc::new(FetchTransferProgress::default());
            let prepared = prepare_snapshot_fetch(
                &request.fetch_request,
                &download_client,
                &progress,
                &transfer_progress,
            )?;
            let _promotion_lock = prepare_installation_destination(
                &prepared.snapshot_manifest,
                &request.destination_dir,
                request.staging_dir.as_deref(),
                &request.fetch_request.repo_root,
                &request.fetch_request.control,
            )?;
            let source = LoadedSnapshotSource::load(
                &prepared.repo_paths,
                request.reuse_snapshot_id.as_deref(),
                request.reuse_from.as_deref(),
                &request.destination_dir,
            )?;
            let reconstruction = prepare_reconstruction(ReconstructInputs {
                snapshot_manifest: &prepared.snapshot_manifest,
                chunk_index: &prepared.chunk_index,
                repo_paths: &prepared.repo_paths,
                existing_artifacts: &prepared.existing_artifacts,
                source: source.as_ref().map(LoadedSnapshotSource::as_source),
                runtime_hints: &request.runtime_hints,
                control: &request.fetch_request.control,
            })?;
            let pending_artifact_downloads = select_pending_artifact_downloads(
                &prepared.pending_artifact_downloads,
                &reconstruction.plan.required_artifacts,
            );
            if pending_artifact_downloads.is_empty() {
                transfer_progress.finish();
            }

            let (reconstruct_result, _) = apply_reconstruct_pipeline(
                ReconstructPipelineApplyRequest {
                    publication: Publication::ReplaceDirectory,
                    repo_root: &prepared.repo_paths.repo_root,
                    snapshot_manifest: &prepared.snapshot_manifest,
                    destination_dir: &request.destination_dir,
                    staging_dir: request.staging_dir.as_deref(),
                    source_destination_dir: reconstruction.source_directory,
                    runtime_hints: &request.runtime_hints,
                    existing_artifacts: &prepared.existing_artifacts,
                    pending_artifact_downloads: &pending_artifact_downloads,
                    transfer_policy: request.fetch_request.transfer_policy,
                    control: &request.fetch_request.control,
                    download_client: Some(&download_client),
                    transfer_progress: Some(&transfer_progress),
                },
                reconstruction.plan,
            )?;
            publish_fetched_snapshot(&prepared, &request.fetch_request.control)?;
            let transfer_stats = transfer_progress.stats(request.fetch_request.transfer_policy);
            let fetch_result = build_fetch_result(
                &request.fetch_request,
                reconstruct_result
                    .reconstruct_result
                    .required_artifact_count,
                reconstruct_result.reconstruct_result.required_bundle_count,
                pending_artifact_downloads.len(),
                pending_artifact_downloads
                    .iter()
                    .filter(|download| download.artifact.is_bundle())
                    .count(),
                transfer_stats,
            );

            Ok(InstallationFetchAndReconstructResult {
                fetch_result,
                reconstruct_result,
            })
        },
    )
}

//! Combined download and reconstruction, preserving one repository lock and pipeline.
use crate::Result;
use crate::installation::execute::apply_reconstruct_pipeline;
use crate::installation::jobs::ReconstructPipelineApplyRequest;
use crate::installation::prepare::{ReconstructionInputs, prepare_reconstruction};
use crate::installation::types::{InstallationReconstructResult, SnapshotReconstructSource};
use crate::manifest::snapshot::SnapshotManifest;
use crate::runtime::RuntimeResourceHints;
use crate::sync::fetch::{
    build_fetch_result, prepare_snapshot_fetch, publish_fetched_snapshot,
    select_pending_artifact_downloads,
};
use crate::sync::fetch_types::{SnapshotFetchRequest, SnapshotFetchResult};
use crate::transfer::http::build_download_client;
use crate::transfer::progress::FetchTransferProgress;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone)]
pub struct SnapshotFetchAndReconstructRequest {
    pub fetch_request: SnapshotFetchRequest,
    pub destination_dir: PathBuf,
    pub staging_dir: Option<PathBuf>,
    pub source_snapshot_id: Option<String>,
    pub source_dir: Option<PathBuf>,
    pub runtime_hints: RuntimeResourceHints,
}

#[derive(Debug, Clone)]
pub struct SnapshotFetchAndReconstructResult {
    pub fetch_result: SnapshotFetchResult,
    pub reconstruct_result: InstallationReconstructResult,
}

struct ResolvedSnapshotSource {
    source_manifest: Option<SnapshotManifest>,
    source_destination_dir: Option<PathBuf>,
}

pub fn fetch_and_reconstruct_snapshot(
    request: SnapshotFetchAndReconstructRequest,
) -> Result<SnapshotFetchAndReconstructResult> {
    request.fetch_request.control.checkpoint()?;
    let operation_root = request.fetch_request.repo_path.to_path_buf();
    std::fs::create_dir_all(&operation_root)?;
    crate::repo::coordination::write_with_control(
        &operation_root,
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
            let _promotion_lock = crate::installation::apply::prepare_installation_destination(
                &prepared.snapshot_manifest,
                &request.destination_dir,
                request.staging_dir.as_deref(),
                &request.fetch_request.repo_path,
                &request.fetch_request.control,
            )?;
            let resolved_source = resolve_snapshot_source(&request, &prepared.repo_paths)?;
            let reconstruction = prepare_reconstruction(ReconstructionInputs {
                snapshot_manifest: &prepared.snapshot_manifest,
                chunk_index: &prepared.chunk_index,
                repo_paths: &prepared.repo_paths,
                existing_artifacts: &prepared.existing_artifacts,
                source: resolved_source
                    .source_manifest
                    .as_ref()
                    .zip(resolved_source.source_destination_dir.as_deref())
                    .map(
                        |(snapshot_manifest, destination_dir)| SnapshotReconstructSource {
                            snapshot_manifest,
                            destination_dir,
                        },
                    ),
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
                    publication: crate::installation::publish::Publication::ReplaceDirectory,
                    repo_root: &prepared.repo_paths.repo_root,
                    compression_profile: prepared.compression_profile.clone(),
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
            publish_fetched_snapshot(&prepared)?;
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
                    .filter(|download| {
                        matches!(
                            download.artifact,
                            crate::artifact::reference::ArtifactRef::Bundle { .. }
                        )
                    })
                    .count(),
                transfer_stats,
            );

            Ok(SnapshotFetchAndReconstructResult {
                fetch_result,
                reconstruct_result,
            })
        },
    )
}

fn resolve_snapshot_source(
    request: &SnapshotFetchAndReconstructRequest,
    repo_paths: &crate::repo::paths::RepoPaths,
) -> Result<ResolvedSnapshotSource> {
    let source_manifest = match request.source_snapshot_id.as_ref() {
        Some(source_snapshot_id) => Some(crate::repo::snapshot_manifests::load_snapshot_manifest(
            repo_paths,
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
    Ok(ResolvedSnapshotSource {
        source_manifest,
        source_destination_dir,
    })
}

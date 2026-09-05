use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use super::apply::resolve_verified_source;
use super::pipeline::{
    ReconstructPipelineApplyRequest, apply_reconstruct_pipeline, build_reconstruct_pipeline_plan,
};
use super::transfer::{
    ArtifactDownloadEvent, ArtifactDownloadScheduler, ArtifactPriority, FetchTransferProgress,
    MAX_CHUNK_INDEX_BYTES, MAX_SNAPSHOT_BLUEPRINT_BYTES, MAX_SNAPSHOT_METADATA_BYTES,
    PendingBundleDownload, TransferDownloadRequest, TransferPhaseCounters, build_download_client,
    fetch_bundle_urls, snapshot_manifest_from_download, write_repo_manifest,
};
use super::types::{
    SnapshotDownloadAndReconstructRequest, SnapshotDownloadAndReconstructResult,
    SnapshotFetchRequest, SnapshotFetchResult, SnapshotReconstructSource,
};
use crate::manifest::gutch_repo::GutchRepoManifest;
use crate::manifest::snapshot::SnapshotManifest;
use crate::progress::{
    GutchOperation, GutchProgressEvent, GutchProgressPhase, GutchProgressSink, emit_progress,
};
use crate::{Error, Result};

struct PreparedSnapshotFetch {
    repo_paths: crate::GutchRepoPaths,
    compression_profile: crate::manifest::CompressionProfile,
    snapshot_manifest: SnapshotManifest,
    chunk_index: crate::manifest::ChunkIndex,
    required_artifacts: HashSet<crate::ArtifactRef>,
    existing_artifacts: HashSet<crate::ArtifactRef>,
    pending_bundle_downloads: Vec<PendingBundleDownload>,
}

struct ResolvedSnapshotSource {
    source_manifest: Option<SnapshotManifest>,
    source_destination_dir: Option<PathBuf>,
}

pub fn download_and_reconstruct_snapshot(
    request: SnapshotDownloadAndReconstructRequest,
) -> Result<SnapshotDownloadAndReconstructResult> {
    download_and_apply_unified_snapshot_ops(request)
}

pub fn fetch_snapshot(request: &SnapshotFetchRequest) -> Result<SnapshotFetchResult> {
    fetch_snapshot_for_required_bundle_ids(request, None)
}

pub fn fetch_snapshot_for_required_bundle_ids(
    request: &SnapshotFetchRequest,
    required_bundle_ids_override: Option<&HashSet<String>>,
) -> Result<SnapshotFetchResult> {
    request.control.checkpoint()?;
    let _operation_span = request.control.profile_span("snapshot.fetch");
    let progress = request.control.progress().clone();
    let download_client = build_download_client()?;
    let transfer_progress = Arc::new(FetchTransferProgress::default());
    let prepared =
        prepare_snapshot_fetch(request, &download_client, &progress, &transfer_progress)?;
    let required_artifacts = required_bundle_ids_override
        .map(|ids| {
            prepared
                .required_artifacts
                .iter()
                .filter(|artifact| ids.contains(artifact.id()))
                .cloned()
                .collect::<HashSet<_>>()
        })
        .unwrap_or_else(|| prepared.required_artifacts.clone());
    let pending_bundle_downloads =
        select_pending_bundle_downloads(&prepared.pending_bundle_downloads, &required_artifacts);
    let downloaded_artifact_count = download_pending_bundle_files(
        request,
        &download_client,
        &transfer_progress,
        &pending_bundle_downloads,
    )?;
    transfer_progress.finish();
    let transfer_stats = transfer_progress.stats(request.transfer_policy);
    let required_bundle_count = required_artifacts
        .iter()
        .filter(|artifact| matches!(artifact, crate::ArtifactRef::Bundle { .. }))
        .count();
    let downloaded_bundle_count = pending_bundle_downloads
        .iter()
        .filter(|download| matches!(download.artifact, crate::ArtifactRef::Bundle { .. }))
        .count();

    emit_progress(
        &progress,
        GutchProgressEvent {
            operation: GutchOperation::Fetch,
            phase: GutchProgressPhase::Done,
            message: "Snapshot fetched".to_string(),
            current: required_artifacts.len(),
            total: required_artifacts.len().max(1),
            transferred_bytes: transfer_progress.transferred_bytes(),
            total_bytes: transfer_progress.total_bytes(),
            detail: Some(request.snapshot_id.clone()),
            done: true,
        },
    );

    Ok(build_fetch_result(
        &prepared.repo_paths.repo_root,
        &request.snapshot_id,
        required_artifacts.len(),
        required_bundle_count,
        downloaded_artifact_count,
        downloaded_bundle_count,
        request.transfer_policy,
        transfer_stats,
    ))
}

fn download_and_apply_unified_snapshot_ops(
    request: SnapshotDownloadAndReconstructRequest,
) -> Result<SnapshotDownloadAndReconstructResult> {
    request.fetch_request.control.checkpoint()?;
    let _operation_span = request
        .fetch_request
        .control
        .profile_span("snapshot.fetch_reconstruct");
    let progress = request.fetch_request.control.progress().clone();
    let download_client = build_download_client()?;
    let transfer_progress = Arc::new(FetchTransferProgress::default());
    let prepared = prepare_snapshot_fetch(
        &request.fetch_request,
        &download_client,
        &progress,
        &transfer_progress,
    )?;
    let resolved_source = resolve_snapshot_source(&request, &prepared.repo_paths)?;
    let verified_source = resolve_verified_source(
        resolved_source
            .source_manifest
            .as_ref()
            .zip(resolved_source.source_destination_dir.as_deref())
            .map(
                |(source_manifest, source_destination_dir)| SnapshotReconstructSource {
                    snapshot_manifest: source_manifest,
                    destination_dir: source_destination_dir,
                },
            ),
        &request.runtime_hints,
        &request.fetch_request.control,
    )?;

    let pipeline_plan = {
        let _plan_span = request
            .fetch_request
            .control
            .profile_span("snapshot.reconstruct.prepare.plan");
        build_reconstruct_pipeline_plan(
            &prepared.snapshot_manifest,
            &prepared.chunk_index,
            &prepared.repo_paths,
            &prepared.existing_artifacts,
            verified_source,
        )?
    };
    let pending_bundle_downloads = select_pending_bundle_downloads(
        &prepared.pending_bundle_downloads,
        &pipeline_plan.required_artifacts,
    );
    if pending_bundle_downloads.is_empty() {
        transfer_progress.finish();
    }

    let reconstruct_result = apply_reconstruct_pipeline(
        ReconstructPipelineApplyRequest {
            repo_root: &prepared.repo_paths.repo_root,
            compression_profile: prepared.compression_profile.clone(),
            snapshot_manifest: &prepared.snapshot_manifest,
            destination_dir: &request.destination_dir,
            staging_dir: request.staging_dir.as_deref(),
            source_destination_dir: resolved_source.source_destination_dir,
            runtime_hints: &request.runtime_hints,
            existing_artifacts: &prepared.existing_artifacts,
            pending_bundle_downloads: &pending_bundle_downloads,
            transfer_policy: request.fetch_request.transfer_policy,
            control: &request.fetch_request.control,
            download_client: Some(&download_client),
            transfer_progress: Some(&transfer_progress),
        },
        pipeline_plan,
    )?;
    let transfer_stats = transfer_progress.stats(request.fetch_request.transfer_policy);
    let fetch_result = build_fetch_result(
        &prepared.repo_paths.repo_root,
        &request.fetch_request.snapshot_id,
        reconstruct_result
            .reconstruct_result
            .required_artifact_count,
        reconstruct_result.reconstruct_result.required_bundle_count,
        pending_bundle_downloads.len(),
        pending_bundle_downloads
            .iter()
            .filter(|download| matches!(download.artifact, crate::ArtifactRef::Bundle { .. }))
            .count(),
        request.fetch_request.transfer_policy,
        transfer_stats,
    );

    Ok(SnapshotDownloadAndReconstructResult {
        fetch_result,
        reconstruct_result,
    })
}

fn prepare_snapshot_fetch(
    request: &SnapshotFetchRequest,
    download_client: &reqwest::blocking::Client,
    progress: &Option<Arc<dyn GutchProgressSink>>,
    transfer_progress: &Arc<FetchTransferProgress>,
) -> Result<PreparedSnapshotFetch> {
    request.control.checkpoint()?;
    let _prepare_span = request.control.profile_span("fetch.prepare");
    emit_progress(
        progress,
        GutchProgressEvent {
            operation: GutchOperation::Fetch,
            phase: GutchProgressPhase::Prepare,
            message: "Preparing local repo layout".to_string(),
            current: 0,
            total: 1,
            transferred_bytes: 0,
            total_bytes: 0,
            detail: Some(request.snapshot_id.clone()),
            done: false,
        },
    );

    let repo_paths = crate::GutchRepoPaths::from_repo_root(&request.repo_path);
    crate::ensure_gutch_repo_layout(&repo_paths, "download")?;

    if let Some(repo_metadata_url) = request.repo_metadata_url.as_deref() {
        emit_progress(
            progress,
            GutchProgressEvent {
                operation: GutchOperation::Fetch,
                phase: GutchProgressPhase::RepoMetadata,
                message: "Downloading repo metadata".to_string(),
                current: 0,
                total: 1,
                transferred_bytes: 0,
                total_bytes: 0,
                detail: None,
                done: false,
            },
        );
        let repo_manifest_bytes = TransferDownloadRequest {
            client: download_client,
            url: repo_metadata_url,
            control: &request.control,
            transfer_progress,
            phase: GutchProgressPhase::RepoMetadata,
            message: "Downloading repo metadata",
            counters: TransferPhaseCounters {
                current: 0,
                total: 1,
            },
            detail: None,
        }
        .download_bounded(MAX_SNAPSHOT_METADATA_BYTES)?;
        let repo_manifest: GutchRepoManifest =
            crate::helpers::from_json_bytes(&repo_manifest_bytes)?;
        write_repo_manifest(&repo_paths, &repo_manifest)?;
    } else if !repo_paths.repo_metadata_path.exists() {
        write_repo_manifest(&repo_paths, &GutchRepoManifest::new("download".to_string()))?;
    }

    emit_progress(
        progress,
        GutchProgressEvent {
            operation: GutchOperation::Fetch,
            phase: GutchProgressPhase::SnapshotMetadata,
            message: "Downloading snapshot metadata".to_string(),
            current: 0,
            total: 1,
            transferred_bytes: 0,
            total_bytes: 0,
            detail: Some(request.snapshot_id.clone()),
            done: false,
        },
    );
    let snapshot_manifest_bytes = TransferDownloadRequest {
        client: download_client,
        url: &request.metadata_url,
        control: &request.control,
        transfer_progress,
        phase: GutchProgressPhase::SnapshotMetadata,
        message: "Downloading snapshot metadata",
        counters: TransferPhaseCounters {
            current: 0,
            total: 1,
        },
        detail: Some(request.snapshot_id.as_str()),
    }
    .download_bounded(MAX_SNAPSHOT_METADATA_BYTES)?;
    let snapshot_metadata =
        snapshot_manifest_from_download(&request.snapshot_id, &snapshot_manifest_bytes)?;
    crate::repo::write_snapshot_metadata_only(&repo_paths, &snapshot_metadata)?;

    if let Some(product_metadata_url) = request.product_metadata_url.as_deref() {
        let product_metadata_bytes = TransferDownloadRequest {
            client: download_client,
            url: product_metadata_url,
            control: &request.control,
            transfer_progress,
            phase: GutchProgressPhase::SnapshotMetadata,
            message: "Downloading snapshot product metadata",
            counters: TransferPhaseCounters {
                current: 0,
                total: 1,
            },
            detail: Some(request.snapshot_id.as_str()),
        }
        .download_bounded(MAX_SNAPSHOT_METADATA_BYTES)?;
        let product_metadata: crate::manifest::SnapshotProductMetadata =
            crate::helpers::from_json_bytes(&product_metadata_bytes)?;
        product_metadata.validate_structure()?;
        crate::repo::write_snapshot_product_metadata(
            &repo_paths,
            &request.snapshot_id,
            Some(&product_metadata),
        )?;
    } else {
        crate::repo::write_snapshot_product_metadata(&repo_paths, &request.snapshot_id, None)?;
    }

    let blueprint_bytes = TransferDownloadRequest {
        client: download_client,
        url: &request.blueprint_url,
        control: &request.control,
        transfer_progress,
        phase: GutchProgressPhase::SnapshotMetadata,
        message: "Downloading snapshot blueprint",
        counters: TransferPhaseCounters {
            current: 0,
            total: 1,
        },
        detail: Some(request.snapshot_id.as_str()),
    }
    .download_bounded(MAX_SNAPSHOT_BLUEPRINT_BYTES)?;
    let blueprint_path = crate::snapshot_blueprint_path(&repo_paths, &request.snapshot_id)?;
    crate::helpers::write(&blueprint_path, &blueprint_bytes)?;
    let snapshot_manifest = crate::load_snapshot_manifest(&repo_paths, &request.snapshot_id)?;

    if let Some(chunk_index_url) = request.chunk_index_url.as_deref() {
        emit_progress(
            progress,
            GutchProgressEvent {
                operation: GutchOperation::Fetch,
                phase: GutchProgressPhase::ChunkIndex,
                message: "Downloading chunk index".to_string(),
                current: 0,
                total: 1,
                transferred_bytes: 0,
                total_bytes: 0,
                detail: None,
                done: false,
            },
        );
        let chunk_index_bytes = TransferDownloadRequest {
            client: download_client,
            url: chunk_index_url,
            control: &request.control,
            transfer_progress,
            phase: GutchProgressPhase::ChunkIndex,
            message: "Downloading chunk index",
            counters: TransferPhaseCounters {
                current: 0,
                total: 1,
            },
            detail: None,
        }
        .download_bounded(MAX_CHUNK_INDEX_BYTES)?;
        let chunk_index: crate::manifest::ChunkIndex =
            match crate::helpers::from_json_bytes(&chunk_index_bytes) {
                Ok(index) => index,
                Err(json_error) => crate::manifest::binary::load_chunk_index_binary_from_bytes(
                    &chunk_index_bytes,
                )
                .map_err(|binary_error| {
                    Error::manifest(format!(
                        "chunk index is neither valid JSON ({json_error}) nor valid Gutch binary ({binary_error})"
                    ))
                })?,
            };
        crate::write_chunk_index(&repo_paths, &chunk_index)?;
    }

    request.control.checkpoint()?;
    let chunk_index = crate::load_chunk_index(&repo_paths)?;
    let (_, repo_manifest) = crate::load_gutch_repo_manifest(&repo_paths.repo_root)?;
    let compression_profile = repo_manifest.compression_profile();
    let required_artifacts = crate::required_artifacts(&snapshot_manifest, &chunk_index)?;
    let mut existing_artifacts = HashSet::new();
    for artifact in &required_artifacts {
        if artifact.local_path(&repo_paths)?.is_file() {
            existing_artifacts.insert(artifact.clone());
        }
    }
    let required_artifact_ids = required_artifacts
        .iter()
        .map(|artifact| artifact.id().to_string())
        .collect::<HashSet<_>>();
    let existing_artifact_ids = existing_artifacts
        .iter()
        .map(|artifact| artifact.id().to_string())
        .collect::<HashSet<_>>();
    let bundle_urls = fetch_bundle_urls(request, &required_artifact_ids, &existing_artifact_ids)?;
    let pending_bundle_downloads = build_pending_bundle_downloads(
        &required_artifacts,
        &existing_artifacts,
        &bundle_urls,
        &repo_paths,
    )?;

    Ok(PreparedSnapshotFetch {
        repo_paths,
        compression_profile,
        snapshot_manifest,
        chunk_index,
        required_artifacts,
        existing_artifacts,
        pending_bundle_downloads,
    })
}

fn build_pending_bundle_downloads(
    required_artifacts: &HashSet<crate::ArtifactRef>,
    existing_artifacts: &HashSet<crate::ArtifactRef>,
    bundle_urls: &HashMap<String, String>,
    repo_paths: &crate::GutchRepoPaths,
) -> Result<Vec<PendingBundleDownload>> {
    let total_missing_bundle_count = required_artifacts
        .len()
        .saturating_sub(existing_artifacts.len());
    let mut pending_artifacts = required_artifacts
        .iter()
        .filter(|artifact| !existing_artifacts.contains(*artifact))
        .cloned()
        .collect::<Vec<_>>();
    pending_artifacts.sort_by(|left, right| left.id().cmp(right.id()));
    pending_artifacts
        .into_iter()
        .enumerate()
        .map(|(ordinal, artifact)| {
            let url = bundle_urls.get(artifact.id()).ok_or_else(|| {
                Error::manifest(format!(
                    "missing download URL for required artifact '{}'",
                    artifact.id()
                ))
            })?;
            let bundle_id = artifact.id().to_string();
            let bundle_path = artifact.local_path(repo_paths)?;
            Ok(PendingBundleDownload {
                artifact,
                bundle_id,
                url: url.clone(),
                bundle_path,
                ordinal,
                total_missing_bundle_count,
            })
        })
        .collect()
}

fn resolve_snapshot_source(
    request: &SnapshotDownloadAndReconstructRequest,
    repo_paths: &crate::GutchRepoPaths,
) -> Result<ResolvedSnapshotSource> {
    let source_manifest = match request.source_snapshot_id.as_ref() {
        Some(source_snapshot_id) => Some(crate::load_snapshot_manifest(
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

fn select_pending_bundle_downloads(
    pending_bundle_downloads: &[PendingBundleDownload],
    required_artifacts: &HashSet<crate::ArtifactRef>,
) -> Vec<PendingBundleDownload> {
    pending_bundle_downloads
        .iter()
        .filter(|download| required_artifacts.contains(&download.artifact))
        .cloned()
        .collect()
}

fn build_fetch_result(
    repo_root: &std::path::Path,
    snapshot_id: &str,
    required_artifact_count: usize,
    required_bundle_count: usize,
    downloaded_artifact_count: usize,
    downloaded_bundle_count: usize,
    transfer_policy: super::types::TransferConcurrencyPolicy,
    transfer_stats: super::types::TransferStats,
) -> SnapshotFetchResult {
    SnapshotFetchResult {
        repo_path: repo_root.to_path_buf(),
        snapshot_id: snapshot_id.to_string(),
        required_bundle_count,
        downloaded_bundle_count,
        skipped_existing_bundle_count: required_bundle_count
            .saturating_sub(downloaded_bundle_count),
        required_artifact_count,
        downloaded_artifact_count,
        skipped_existing_artifact_count: required_artifact_count
            .saturating_sub(downloaded_artifact_count),
        transfer_policy,
        transfer_stats,
    }
}

fn download_pending_bundle_files(
    request: &SnapshotFetchRequest,
    download_client: &reqwest::blocking::Client,
    transfer_progress: &Arc<FetchTransferProgress>,
    pending_bundle_downloads: &[PendingBundleDownload],
) -> Result<usize> {
    if pending_bundle_downloads.is_empty() {
        return Ok(0);
    }
    let _queue_span = request.control.profile_span("fetch.download_queue");

    let priorities = pending_bundle_downloads
        .iter()
        .map(|download| (download.artifact.clone(), ArtifactPriority::default()))
        .collect();
    let scheduler = ArtifactDownloadScheduler::new(
        pending_bundle_downloads,
        priorities,
        request.transfer_policy,
        download_client,
        &request.control,
        Arc::clone(transfer_progress),
    )?;
    let worker_count = scheduler.worker_count();
    let mut completed_workers = 0usize;
    let mut downloaded_count = 0usize;
    loop {
        match scheduler.recv() {
            Ok(ArtifactDownloadEvent::Ready(_)) => downloaded_count += 1,
            Ok(ArtifactDownloadEvent::Failed(error)) => {
                scheduler.cancel();
                scheduler.join()?;
                return Err(error);
            }
            Ok(ArtifactDownloadEvent::WorkerDone) => {
                completed_workers += 1;
                if completed_workers == worker_count {
                    break;
                }
            }
            Err(_) => {
                scheduler.cancel();
                scheduler.join()?;
                return Err(Error::reconstruct(format!(
                    "artifact download scheduler channel closed unexpectedly after {downloaded_count}/{} downloads; {completed_workers}/{worker_count} workers reported completion",
                    pending_bundle_downloads.len(),
                )));
            }
        }
    }
    scheduler.join()?;
    transfer_progress.finish();
    Ok(downloaded_count)
}

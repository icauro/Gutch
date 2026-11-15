//! Snapshot acquisition, validation and repository publication.
const MAX_SNAPSHOT_METADATA_BYTES: u64 = 16 * 1024 * 1024;
const MAX_SNAPSHOT_BLUEPRINT_BYTES: u64 = 256 * 1024 * 1024;
const MAX_CHUNK_INDEX_BYTES: u64 = 256 * 1024 * 1024;

use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;

use crate::Error;
use crate::Result;
use crate::artifact::profiles::CompressionProfile;
use crate::artifact::reference::ArtifactRef;
use crate::filesystem;
use crate::filesystem::constraints::validate_paths;
use crate::filesystem::from_json_bytes;
use crate::manifest::SnapshotProductMetadata;
use crate::manifest::repo_manifest::RepoManifest;
use crate::manifest::snapshot::SnapshotManifest;
use crate::progress::OperationKind;
use crate::progress::ProgressEvent;
use crate::progress::ProgressPhase;
use crate::progress::ProgressSink;
use crate::progress::emit_progress;
use crate::repo::artifacts::required_artifacts;
use crate::repo::artifacts::verify_bundle_identity_file;
use crate::repo::coordination::write_with_control;
use crate::repo::index::ChunkIndex;
use crate::repo::index::binary_codec::load_chunk_index_binary_from_bytes;
use crate::repo::index::load_chunk_index;
use crate::repo::index::preserve_local_chunk_locations;
use crate::repo::index::write_chunk_index;
use crate::repo::paths::RepoPaths;
use crate::repo::paths::ensure_gutch_repo_layout;
use crate::repo::paths::load_gutch_repo_manifest;
use crate::repo::paths::write_gutch_repo_manifest as write_repo_manifest;
use crate::repo::snapshot_manifests::load_snapshot_manifest;
use crate::repo::snapshot_manifests::snapshot_blueprint_path;
use crate::repo::snapshot_manifests::snapshot_manifest_from_download;
use crate::repo::snapshot_manifests::snapshot_manifest_from_parts;
use crate::repo::snapshot_manifests::write_snapshot_manifest_artifacts;
use crate::repo::verify::RepoArtifactVerification;
use crate::repo::verify::verify_snapshot_artifacts;
use crate::snapshot::identity::compute_snapshot_id_from_entries;
use crate::sync::fetch_types::SnapshotFetchRequest;
use crate::sync::fetch_types::SnapshotFetchResult;
use crate::transfer::artifact_download::TransferDownloadRequest;
use crate::transfer::download::ArtifactDownloadEvent;
use crate::transfer::download::ArtifactDownloadScheduler;
use crate::transfer::download::ArtifactPriority;
use crate::transfer::download::PendingArtifactDownload;
use crate::transfer::http::DownloadClient;
use crate::transfer::http::build_download_client;
use crate::transfer::progress::FetchTransferProgress;
use crate::transfer::progress::TransferPhaseCounters;
use crate::transfer::sources::normalize_artifact_download;
use crate::transfer::sources::normalize_artifact_downloads;
use crate::transfer::types::TransferStats;
use std::fs;

pub(crate) struct PreparedSnapshotFetch {
    pub(crate) repo_paths: RepoPaths,
    pub(crate) repo_manifest: RepoManifest,
    pub(crate) compression_profile: CompressionProfile,
    pub(crate) snapshot_manifest: SnapshotManifest,
    pub(crate) chunk_index: ChunkIndex,
    pub(crate) required_artifacts: HashSet<ArtifactRef>,
    pub(crate) existing_artifacts: HashSet<ArtifactRef>,
    pub(crate) pending_artifact_downloads: Vec<PendingArtifactDownload>,
}

pub fn fetch_snapshot(request: &SnapshotFetchRequest) -> Result<SnapshotFetchResult> {
    fetch_snapshot_for_artifact_ids(request, None)
}

pub fn fetch_snapshot_for_artifact_ids(
    request: &SnapshotFetchRequest,
    required_artifact_ids_override: Option<&HashSet<String>>,
) -> Result<SnapshotFetchResult> {
    request.control.checkpoint()?;
    let operation_root = request.repo_path.to_path_buf();
    fs::create_dir_all(&operation_root)?;
    write_with_control(&operation_root, &request.control.clone(), || {
        request.control.checkpoint()?;
        let _operation_span = request.control.profile_span("transfer.fetch");
        let progress = request.control.progress().clone();
        let download_client = build_download_client()?;
        let transfer_progress = Arc::new(FetchTransferProgress::default());
        let prepared =
            prepare_snapshot_fetch(request, &download_client, &progress, &transfer_progress)?;
        let required_artifacts = required_artifact_ids_override
            .map(|ids| {
                prepared
                    .required_artifacts
                    .iter()
                    .filter(|artifact| ids.contains(artifact.id()))
                    .cloned()
                    .collect::<HashSet<_>>()
            })
            .unwrap_or_else(|| prepared.required_artifacts.clone());
        let pending_artifact_downloads = select_pending_artifact_downloads(
            &prepared.pending_artifact_downloads,
            &required_artifacts,
        );
        let downloaded_artifact_count = download_pending_artifacts(
            request,
            &download_client,
            &transfer_progress,
            &pending_artifact_downloads,
        )?;
        publish_fetched_snapshot(&prepared)?;
        let transfer_stats = transfer_progress.stats(request.transfer_policy);
        let required_bundle_count = required_artifacts
            .iter()
            .filter(|artifact| matches!(artifact, ArtifactRef::Bundle { .. }))
            .count();
        let downloaded_bundle_count = pending_artifact_downloads
            .iter()
            .filter(|download| matches!(download.artifact, ArtifactRef::Bundle { .. }))
            .count();

        emit_progress(
            &progress,
            ProgressEvent {
                feedback: None,
                operation: OperationKind::Fetch,
                phase: ProgressPhase::Done,
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
            request,
            required_artifacts.len(),
            required_bundle_count,
            downloaded_artifact_count,
            downloaded_bundle_count,
            transfer_stats,
        ))
    })
}

pub(crate) fn publish_fetched_snapshot(prepared: &PreparedSnapshotFetch) -> Result<()> {
    // Immutable downloads finish first; the validated snapshot is the commit marker.
    write_repo_manifest(&prepared.repo_paths, &prepared.repo_manifest)?;
    write_chunk_index(&prepared.repo_paths, &prepared.chunk_index)?;
    write_snapshot_manifest_artifacts(&prepared.repo_paths, &prepared.snapshot_manifest)?;
    Ok(())
}

pub(crate) fn prepare_snapshot_fetch(
    request: &SnapshotFetchRequest,
    download_client: &DownloadClient,
    progress: &Option<Arc<dyn ProgressSink>>,
    transfer_progress: &Arc<FetchTransferProgress>,
) -> Result<PreparedSnapshotFetch> {
    request.control.checkpoint()?;
    let _prepare_span = request.control.profile_span("transfer.prepare");
    emit_progress(
        progress,
        ProgressEvent {
            feedback: None,
            operation: OperationKind::Fetch,
            phase: ProgressPhase::Prepare,
            message: "Preparing local repo layout".to_string(),
            current: 0,
            total: 1,
            transferred_bytes: 0,
            total_bytes: 0,
            detail: Some(request.snapshot_id.clone()),
            done: false,
        },
    );

    let repo_paths = RepoPaths::from_repo_root(&request.repo_path);
    let repository_existed = repo_paths.repo_metadata_path.exists();
    ensure_gutch_repo_layout(&repo_paths, "download")?;

    let repo_manifest = if let Some(repo_metadata_url) = request.repo_metadata_url.as_deref() {
        emit_progress(
            progress,
            ProgressEvent {
                feedback: None,
                operation: OperationKind::Fetch,
                phase: ProgressPhase::RepoMetadata,
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
            phase: ProgressPhase::RepoMetadata,
            message: "Downloading repo metadata",
            counters: TransferPhaseCounters {
                current: 0,
                total: 1,
            },
            detail: None,
        }
        .download_bounded(MAX_SNAPSHOT_METADATA_BYTES)?;
        let repo_manifest: RepoManifest = from_json_bytes(&repo_manifest_bytes)?;
        repo_manifest
    } else {
        load_gutch_repo_manifest(&repo_paths.repo_root)?.1
    };

    emit_progress(
        progress,
        ProgressEvent {
            feedback: None,
            operation: OperationKind::Fetch,
            phase: ProgressPhase::SnapshotMetadata,
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
        phase: ProgressPhase::SnapshotMetadata,
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

    let product_metadata =
        if let Some(product_metadata_url) = request.product_metadata_url.as_deref() {
            let product_metadata_bytes = TransferDownloadRequest {
                client: download_client,
                url: product_metadata_url,
                control: &request.control,
                transfer_progress,
                phase: ProgressPhase::SnapshotMetadata,
                message: "Downloading snapshot product metadata",
                counters: TransferPhaseCounters {
                    current: 0,
                    total: 1,
                },
                detail: Some(request.snapshot_id.as_str()),
            }
            .download_bounded(MAX_SNAPSHOT_METADATA_BYTES)?;
            Some(SnapshotProductMetadata::from_json_bytes(
                &product_metadata_bytes,
            )?)
        } else {
            None
        };

    let blueprint_bytes = TransferDownloadRequest {
        client: download_client,
        url: &request.blueprint_url,
        control: &request.control,
        transfer_progress,
        phase: ProgressPhase::SnapshotMetadata,
        message: "Downloading snapshot blueprint",
        counters: TransferPhaseCounters {
            current: 0,
            total: 1,
        },
        detail: Some(request.snapshot_id.as_str()),
    }
    .download_bounded(MAX_SNAPSHOT_BLUEPRINT_BYTES)?;
    let mut snapshot_manifest =
        snapshot_manifest_from_parts(snapshot_metadata, product_metadata, &blueprint_bytes)?;

    let incoming_constraints = repo_manifest.constraints.clone().unwrap_or_default();
    incoming_constraints
        .validate_snapshot_tags(&snapshot_manifest.snapshot_tags, &request.snapshot_id)?;
    validate_paths(
        &incoming_constraints.targets,
        snapshot_manifest.entries.iter().map(|entry| entry.path()),
        "fetch",
        Some(&request.snapshot_id),
    )?;
    if repository_existed {
        let local_constraints = load_gutch_repo_manifest(&repo_paths.repo_root)?
            .1
            .constraints
            .unwrap_or_default();
        if local_constraints != incoming_constraints {
            return Err(Error::manifest(
                "incoming repository constraints differ; explicitly update local constraints before fetching",
            ));
        }
    }

    // Equivalent installations can have different blueprints. Preserve a complete
    // cached representation instead of replacing it merely because packaging differs.
    let local_blueprint = snapshot_blueprint_path(&repo_paths, &request.snapshot_id)?;
    if local_blueprint.is_file() && filesystem::read(&local_blueprint)? != blueprint_bytes {
        let local = load_snapshot_manifest(&repo_paths, &request.snapshot_id)?;
        let local_index = load_chunk_index(&repo_paths)?;
        if compute_snapshot_id_from_entries(&local.entries) == request.snapshot_id
            && verify_snapshot_artifacts(
                &repo_paths,
                &local_index,
                &repo_manifest.compression_profile(),
                &request.snapshot_id,
                RepoArtifactVerification::Identity,
            )
            .is_ok_and(|result| result.ok)
        {
            snapshot_manifest = local;
        }
    }

    let mut chunk_index = if let Some(chunk_index_url) = request.chunk_index_url.as_deref() {
        emit_progress(
            progress,
            ProgressEvent {
                feedback: None,
                operation: OperationKind::Fetch,
                phase: ProgressPhase::ChunkIndex,
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
            phase: ProgressPhase::ChunkIndex,
            message: "Downloading chunk index",
            counters: TransferPhaseCounters {
                current: 0,
                total: 1,
            },
            detail: None,
        }
        .download_bounded(MAX_CHUNK_INDEX_BYTES)?;

        load_chunk_index_binary_from_bytes(&chunk_index_bytes)?
    } else {
        load_chunk_index(&repo_paths)?
    };
    preserve_local_chunk_locations(&repo_paths, &mut chunk_index)?;
    request.control.checkpoint()?;
    let compression_profile = repo_manifest.compression_profile();
    let required_artifacts = required_artifacts(&snapshot_manifest, &chunk_index)?;
    let mut existing_artifacts = HashSet::new();
    for artifact in &required_artifacts {
        let path = artifact.local_path(&repo_paths)?;
        if path.is_file() {
            if let ArtifactRef::Bundle { id } = artifact {
                match verify_bundle_identity_file(&path, id) {
                    Ok(()) => {}
                    Err(Error::InvalidBundle(_) | Error::HashMismatch { .. }) => {
                        fs::remove_file(&path)?;
                        continue;
                    }
                    Err(error) => return Err(error),
                }
            }
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
    let artifact_urls =
        fetch_artifact_urls(request, &required_artifact_ids, &existing_artifact_ids)?;
    let pending_artifact_downloads = build_pending_artifact_downloads(
        &required_artifacts,
        &existing_artifacts,
        &artifact_urls,
        &repo_paths,
    )?;

    Ok(PreparedSnapshotFetch {
        repo_paths,
        repo_manifest,
        compression_profile,
        snapshot_manifest,
        chunk_index,
        required_artifacts,
        existing_artifacts,
        pending_artifact_downloads,
    })
}

fn build_pending_artifact_downloads(
    required_artifacts: &HashSet<ArtifactRef>,
    existing_artifacts: &HashSet<ArtifactRef>,
    artifact_urls: &HashMap<String, String>,
    repo_paths: &RepoPaths,
) -> Result<Vec<PendingArtifactDownload>> {
    let missing_artifact_count = required_artifacts
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
            let url = artifact_urls.get(artifact.id()).ok_or_else(|| {
                Error::manifest(format!(
                    "missing download URL for required artifact '{}'",
                    artifact.id()
                ))
            })?;
            let artifact_id = artifact.id().to_string();
            let artifact_path = artifact.local_path(repo_paths)?;
            Ok(PendingArtifactDownload {
                artifact,
                artifact_id,
                url: url.clone(),
                artifact_path,
                ordinal,
                missing_artifact_count,
            })
        })
        .collect()
}

pub(crate) fn select_pending_artifact_downloads(
    pending_artifact_downloads: &[PendingArtifactDownload],
    required_artifacts: &HashSet<ArtifactRef>,
) -> Vec<PendingArtifactDownload> {
    pending_artifact_downloads
        .iter()
        .filter(|download| required_artifacts.contains(&download.artifact))
        .cloned()
        .collect()
}

pub(crate) fn build_fetch_result(
    request: &SnapshotFetchRequest,
    required_artifact_count: usize,
    required_bundle_count: usize,
    downloaded_artifact_count: usize,
    downloaded_bundle_count: usize,
    transfer_stats: TransferStats,
) -> SnapshotFetchResult {
    SnapshotFetchResult {
        repo_path: request.repo_path.clone(),
        snapshot_id: request.snapshot_id.clone(),
        required_bundle_count,
        downloaded_bundle_count,
        skipped_existing_bundle_count: required_bundle_count
            .saturating_sub(downloaded_bundle_count),
        required_artifact_count,
        downloaded_artifact_count,
        skipped_existing_artifact_count: required_artifact_count
            .saturating_sub(downloaded_artifact_count),
        transfer_policy: request.transfer_policy,
        transfer_stats,
    }
}

fn download_pending_artifacts(
    request: &SnapshotFetchRequest,
    download_client: &DownloadClient,
    transfer_progress: &Arc<FetchTransferProgress>,
    pending_artifact_downloads: &[PendingArtifactDownload],
) -> Result<usize> {
    if pending_artifact_downloads.is_empty() {
        return Ok(0);
    }
    let _queue_span = request.control.profile_span("transfer.download_queue");

    let priorities = pending_artifact_downloads
        .iter()
        .map(|download| (download.artifact.clone(), ArtifactPriority::default()))
        .collect();
    let scheduler = ArtifactDownloadScheduler::new(
        pending_artifact_downloads,
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
                    pending_artifact_downloads.len(),
                )));
            }
        }
    }
    scheduler.join()?;
    transfer_progress.finish();
    request.control.download_finished();
    Ok(downloaded_count)
}

pub(crate) fn fetch_artifact_urls(
    request: &SnapshotFetchRequest,
    required_artifact_ids: &HashSet<String>,
    existing_artifact_ids: &HashSet<String>,
) -> Result<HashMap<String, String>> {
    let mut artifact_urls = normalize_artifact_downloads(&request.artifacts)?;
    let unresolved_artifact_ids = required_artifact_ids
        .iter()
        .filter(|artifact_id| {
            !existing_artifact_ids.contains(*artifact_id)
                && !artifact_urls.contains_key(artifact_id.as_str())
        })
        .cloned()
        .collect::<Vec<_>>();
    if unresolved_artifact_ids.is_empty() {
        return Ok(artifact_urls);
    }

    let provider = request.artifact_url_provider.as_ref().ok_or_else(|| {
        Error::manifest(
            "artifact URL provider is required when direct artifact URLs are not provided",
        )
    })?;
    let resolved_artifacts =
        provider.resolve_artifact_urls(&unresolved_artifact_ids, &request.control)?;
    for artifact in resolved_artifacts {
        let (artifact_id, url) = normalize_artifact_download(&artifact)?;
        if !unresolved_artifact_ids.iter().any(|id| id == artifact_id) {
            return Err(Error::manifest(format!(
                "artifact URL provider returned unexpected artifact '{}'",
                artifact_id
            )));
        }
        if artifact_urls
            .insert(artifact_id.to_string(), url.to_string())
            .is_some()
        {
            return Err(Error::manifest(format!(
                "artifact URL provider returned duplicate artifact '{}'",
                artifact_id
            )));
        }
    }

    let missing = unresolved_artifact_ids
        .into_iter()
        .filter(|artifact_id| !artifact_urls.contains_key(artifact_id))
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(Error::manifest(format!(
            "missing artifact download URLs for: {}",
            missing.join(", ")
        )));
    }
    Ok(artifact_urls)
}

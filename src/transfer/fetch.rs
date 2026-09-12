use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use crate::Error;
use crate::Result;
use crate::installation::apply::resolve_verified_source;
use crate::installation::execute::apply_reconstruct_pipeline;
use crate::installation::jobs::ReconstructPipelineApplyRequest;
use crate::installation::plan::build_reconstruct_pipeline_plan;
use crate::installation::types::SnapshotReconstructSource;
use crate::manifest::gutch_repo::GutchRepoManifest;
use crate::manifest::snapshot::SnapshotManifest;
use crate::progress::GutchOperation;
use crate::progress::GutchProgressEvent;
use crate::progress::GutchProgressPhase;
use crate::progress::GutchProgressSink;
use crate::progress::emit_progress;
use crate::transfer::download::ArtifactDownloadEvent;
use crate::transfer::download::ArtifactDownloadScheduler;
use crate::transfer::download::ArtifactPriority;
use crate::transfer::download::FetchTransferProgress;
use crate::transfer::download::MAX_CHUNK_INDEX_BYTES;
use crate::transfer::download::MAX_SNAPSHOT_BLUEPRINT_BYTES;
use crate::transfer::download::MAX_SNAPSHOT_METADATA_BYTES;
use crate::transfer::download::PendingArtifactDownload;
use crate::transfer::download::TransferDownloadRequest;
use crate::transfer::download::TransferPhaseCounters;
use crate::transfer::download::build_download_client;
use crate::transfer::download::fetch_bundle_urls;
use crate::transfer::download::snapshot_manifest_from_download;
use crate::transfer::download::write_repo_manifest;
use crate::transfer::types::SnapshotDownloadAndReconstructRequest;
use crate::transfer::types::SnapshotDownloadAndReconstructResult;
use crate::transfer::types::SnapshotFetchRequest;
use crate::transfer::types::SnapshotFetchResult;

struct PreparedSnapshotFetch {
    repo_paths: crate::repo::paths::GutchRepoPaths,
    repo_manifest: GutchRepoManifest,
    compression_profile: crate::manifest::CompressionProfile,
    snapshot_manifest: SnapshotManifest,
    chunk_index: crate::manifest::ChunkIndex,
    required_artifacts: HashSet<crate::repo::artifacts::ArtifactRef>,
    existing_artifacts: HashSet<crate::repo::artifacts::ArtifactRef>,
    pending_artifact_downloads: Vec<PendingArtifactDownload>,
}

struct ResolvedSnapshotSource {
    source_manifest: Option<SnapshotManifest>,
    source_destination_dir: Option<PathBuf>,
}

pub fn fetch_snapshot(request: &SnapshotFetchRequest) -> Result<SnapshotFetchResult> {
    fetch_snapshot_for_required_bundle_ids(request, None)
}

pub fn fetch_snapshot_for_required_bundle_ids(
    request: &SnapshotFetchRequest,
    required_bundle_ids_override: Option<&HashSet<String>>,
) -> Result<SnapshotFetchResult> {
    request.control.checkpoint()?;
    let operation_root = (&request.repo_path).to_path_buf();
    std::fs::create_dir_all(&operation_root)?;
    crate::repo::coordination::write_with_control(&operation_root, &request.control.clone(), || {
        request.control.checkpoint()?;
        let _operation_span = request.control.profile_span("transfer.fetch");
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
        let pending_artifact_downloads = select_pending_artifact_downloads(
            &prepared.pending_artifact_downloads,
            &required_artifacts,
        );
        let downloaded_artifact_count = download_pending_bundle_files(
            request,
            &download_client,
            &transfer_progress,
            &pending_artifact_downloads,
        )?;
        transfer_progress.finish();
        request.control.download_finished();
        publish_fetched_snapshot(&prepared)?;
        let transfer_stats = transfer_progress.stats(request.transfer_policy);
        let required_bundle_count = required_artifacts
            .iter()
            .filter(|artifact| {
                matches!(artifact, crate::repo::artifacts::ArtifactRef::Bundle { .. })
            })
            .count();
        let downloaded_bundle_count = pending_artifact_downloads
            .iter()
            .filter(|download| {
                matches!(
                    download.artifact,
                    crate::repo::artifacts::ArtifactRef::Bundle { .. }
                )
            })
            .count();

        emit_progress(
            &progress,
            GutchProgressEvent {
                feedback: None,
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
    })
}

pub fn download_and_reconstruct_snapshot(
    request: SnapshotDownloadAndReconstructRequest,
) -> Result<SnapshotDownloadAndReconstructResult> {
    request.fetch_request.control.checkpoint()?;
    let operation_root = (&request.fetch_request.repo_path).to_path_buf();
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
            crate::filesystem::paths::validate_destination_paths(
                &request.destination_dir,
                prepared
                    .snapshot_manifest
                    .entries
                    .iter()
                    .map(|entry| entry.path()),
            )?;
            let _promotion_lock = crate::installation::apply::prepare_directory_promotion(
                &request.destination_dir,
                request.staging_dir.as_deref(),
                &request.fetch_request.repo_path,
                &request.fetch_request.control,
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
                    .profile_span("installation.reconstruct.prepare.plan");
                build_reconstruct_pipeline_plan(
                    &prepared.snapshot_manifest,
                    &prepared.chunk_index,
                    &prepared.repo_paths,
                    &prepared.existing_artifacts,
                    verified_source.as_ref().map(|source| source.source()),
                )?
            };
            let pending_artifact_downloads = select_pending_artifact_downloads(
                &prepared.pending_artifact_downloads,
                &pipeline_plan.required_artifacts,
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
                    source_destination_dir: resolved_source.source_destination_dir,
                    runtime_hints: &request.runtime_hints,
                    existing_artifacts: &prepared.existing_artifacts,
                    pending_artifact_downloads: &pending_artifact_downloads,
                    transfer_policy: request.fetch_request.transfer_policy,
                    control: &request.fetch_request.control,
                    download_client: Some(&download_client),
                    transfer_progress: Some(&transfer_progress),
                },
                pipeline_plan,
            )?;
            publish_fetched_snapshot(&prepared)?;
            let transfer_stats = transfer_progress.stats(request.fetch_request.transfer_policy);
            let fetch_result = build_fetch_result(
                &prepared.repo_paths.repo_root,
                &request.fetch_request.snapshot_id,
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
                            crate::repo::artifacts::ArtifactRef::Bundle { .. }
                        )
                    })
                    .count(),
                request.fetch_request.transfer_policy,
                transfer_stats,
            );

            Ok(SnapshotDownloadAndReconstructResult {
                fetch_result,
                reconstruct_result,
            })
        },
    )
}

fn publish_fetched_snapshot(prepared: &PreparedSnapshotFetch) -> Result<()> {
    // Immutable downloads finish first; the validated snapshot is the commit marker.
    write_repo_manifest(&prepared.repo_paths, &prepared.repo_manifest)?;
    crate::repo::index::write_chunk_index(&prepared.repo_paths, &prepared.chunk_index)?;
    crate::repo::snapshot_manifests::write_snapshot_manifest_artifacts(
        &prepared.repo_paths,
        &prepared.snapshot_manifest,
    )?;
    Ok(())
}

fn prepare_snapshot_fetch(
    request: &SnapshotFetchRequest,
    download_client: &reqwest::blocking::Client,
    progress: &Option<Arc<dyn GutchProgressSink>>,
    transfer_progress: &Arc<FetchTransferProgress>,
) -> Result<PreparedSnapshotFetch> {
    request.control.checkpoint()?;
    let _prepare_span = request.control.profile_span("transfer.prepare");
    emit_progress(
        progress,
        GutchProgressEvent {
            feedback: None,
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

    let repo_paths = crate::repo::paths::GutchRepoPaths::from_repo_root(&request.repo_path);
    let repository_existed = repo_paths.repo_metadata_path.exists();
    crate::repo::paths::ensure_gutch_repo_layout(&repo_paths, "download")?;

    let repo_manifest = if let Some(repo_metadata_url) = request.repo_metadata_url.as_deref() {
        emit_progress(
            progress,
            GutchProgressEvent {
                feedback: None,
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
            crate::filesystem::from_json_bytes(&repo_manifest_bytes)?;
        repo_manifest
    } else {
        crate::repo::paths::load_gutch_repo_manifest(&repo_paths.repo_root)?.1
    };

    emit_progress(
        progress,
        GutchProgressEvent {
            feedback: None,
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

    let product_metadata =
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
                crate::filesystem::from_json_bytes(&product_metadata_bytes)?;
            product_metadata.validate_structure()?;
            Some(product_metadata)
        } else {
            None
        };

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
    let mut snapshot_manifest = crate::repo::snapshot_manifests::snapshot_manifest_from_parts(
        snapshot_metadata,
        product_metadata,
        &blueprint_bytes,
    )?;

    let incoming_constraints = repo_manifest.constraints.clone().unwrap_or_default();
    crate::filesystem::constraints::validate_paths(
        &incoming_constraints,
        snapshot_manifest.entries.iter().map(|entry| entry.path()),
        "fetch",
        Some(&request.snapshot_id),
    )?;
    if repository_existed {
        let local_constraints =
            crate::repo::paths::load_gutch_repo_manifest(&repo_paths.repo_root)?
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
    let local_blueprint = crate::repo::snapshot_manifests::snapshot_blueprint_path(
        &repo_paths,
        &request.snapshot_id,
    )?;
    if local_blueprint.is_file() && crate::filesystem::read(&local_blueprint)? != blueprint_bytes {
        let local = crate::repo::snapshot_manifests::load_snapshot_manifest(
            &repo_paths,
            &request.snapshot_id,
        )?;
        let local_index = crate::repo::index::load_chunk_index(&repo_paths)?;
        if crate::snapshot::identity::compute_snapshot_id_from_entries(&local.entries)
            == request.snapshot_id
            && crate::repo::verify::verify_snapshot_artifacts(
                &repo_paths,
                &local_index,
                &repo_manifest.compression_profile(),
                &request.snapshot_id,
                crate::installation::inspect::VerifyMode::Strict,
            )
            .is_ok_and(|result| result.ok)
        {
            snapshot_manifest = local;
        }
    }

    let mut chunk_index = if let Some(chunk_index_url) = request.chunk_index_url.as_deref() {
        emit_progress(
            progress,
            GutchProgressEvent {
                feedback: None,
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
        let chunk_index =
            crate::repo::index::codec::load_chunk_index_binary_from_bytes(&chunk_index_bytes)?;
        chunk_index
    } else {
        crate::repo::index::load_chunk_index(&repo_paths)?
    };
    // Retain cached mappings whose artifacts are available, plus local-only entries.
    for (hash, entry) in crate::repo::index::load_chunk_index(&repo_paths)?.entries {
        let artifact =
            crate::repo::artifacts::ArtifactRef::from_chunk_entry(&hash.to_hex(), &entry);
        if artifact.local_path(&repo_paths)?.is_file()
            || chunk_index.entries.get_hash(&hash).is_none()
        {
            chunk_index.entries.insert(hash, entry);
        }
    }
    request.control.checkpoint()?;
    let compression_profile = repo_manifest.compression_profile();
    let required_artifacts =
        crate::repo::artifacts::required_artifacts(&snapshot_manifest, &chunk_index)?;
    let mut existing_artifacts = HashSet::new();
    for artifact in &required_artifacts {
        let path = artifact.local_path(&repo_paths)?;
        if path.is_file() {
            if let crate::repo::artifacts::ArtifactRef::Bundle { id } = artifact {
                match crate::repo::artifacts::verify_bundle_identity_file(&path, id) {
                    Ok(()) => {}
                    Err(Error::InvalidBundle(_) | Error::HashMismatch { .. }) => {
                        std::fs::remove_file(&path)?;
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
    let bundle_urls = fetch_bundle_urls(request, &required_artifact_ids, &existing_artifact_ids)?;
    let pending_artifact_downloads = build_pending_artifact_downloads(
        &required_artifacts,
        &existing_artifacts,
        &bundle_urls,
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
    required_artifacts: &HashSet<crate::repo::artifacts::ArtifactRef>,
    existing_artifacts: &HashSet<crate::repo::artifacts::ArtifactRef>,
    bundle_urls: &HashMap<String, String>,
    repo_paths: &crate::repo::paths::GutchRepoPaths,
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
            let url = bundle_urls.get(artifact.id()).ok_or_else(|| {
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

fn resolve_snapshot_source(
    request: &SnapshotDownloadAndReconstructRequest,
    repo_paths: &crate::repo::paths::GutchRepoPaths,
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

fn select_pending_artifact_downloads(
    pending_artifact_downloads: &[PendingArtifactDownload],
    required_artifacts: &HashSet<crate::repo::artifacts::ArtifactRef>,
) -> Vec<PendingArtifactDownload> {
    pending_artifact_downloads
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
    transfer_policy: crate::transfer::types::TransferConcurrencyPolicy,
    transfer_stats: crate::transfer::types::TransferStats,
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

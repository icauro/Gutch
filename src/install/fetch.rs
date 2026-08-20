use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use rayon::prelude::*;

use super::apply::resolve_verified_source_install;
use super::pipeline::{
    InstallPipelineApplyRequest, apply_install_pipeline, build_install_pipeline_plan,
};
use super::transfer::{
    FetchTransferProgress, PendingBundleDownload, TransferPhaseCounters, build_download_client,
    bundle_download_worker_count, download_pending_bundle, download_url, fetch_bundle_urls,
    target_manifest_from_download, write_repo_manifest,
};
use super::types::{
    TargetInstallApplySource, TargetInstallDownloadAndApplyRequest,
    TargetInstallDownloadAndApplyResult, TargetInstallFetchRequest, TargetInstallFetchResult,
};
use crate::manifest::gutch_repo::GutchRepoManifest;
use crate::manifest::target::TargetManifest;
use crate::progress::{
    GutchOperation, GutchProgressEvent, GutchProgressPhase, GutchProgressSink, emit_progress,
};
use crate::runtime::install_in_thread_pool;
use crate::{Error, Result};

struct PreparedTargetInstallFetch {
    repo_paths: crate::GutchRepoPaths,
    compression_profile: crate::manifest::CompressionProfile,
    target_manifest: TargetManifest,
    chunk_index: crate::manifest::ChunkIndex,
    required_bundle_ids: HashSet<String>,
    existing_bundle_ids: HashSet<String>,
    pending_bundle_downloads: Vec<PendingBundleDownload>,
}

struct ResolvedInstallSource {
    source_manifest: Option<TargetManifest>,
    source_install_dir: Option<PathBuf>,
}

pub fn download_and_apply_target_install(
    request: TargetInstallDownloadAndApplyRequest,
) -> Result<TargetInstallDownloadAndApplyResult> {
    download_and_apply_unified_target_install(request)
}

pub fn fetch_target_install_content(
    request: &TargetInstallFetchRequest,
) -> Result<TargetInstallFetchResult> {
    fetch_target_install_content_for_required_bundle_ids(request, None)
}

pub fn fetch_target_install_content_for_required_bundle_ids(
    request: &TargetInstallFetchRequest,
    required_bundle_ids_override: Option<&HashSet<String>>,
) -> Result<TargetInstallFetchResult> {
    request.control.checkpoint()?;
    let progress = request.control.progress().clone();
    let download_client = build_download_client()?;
    let transfer_progress = Arc::new(FetchTransferProgress::default());
    let prepared =
        prepare_target_install_fetch(request, &download_client, &progress, &transfer_progress)?;
    let required_bundle_ids = required_bundle_ids_override
        .cloned()
        .unwrap_or_else(|| prepared.required_bundle_ids.clone());
    let pending_bundle_downloads =
        select_pending_bundle_downloads(&prepared.pending_bundle_downloads, &required_bundle_ids);
    let downloaded_bundle_count = download_pending_bundle_files(
        request,
        &download_client,
        &progress,
        &transfer_progress,
        &pending_bundle_downloads,
    )?;

    emit_progress(
        &progress,
        GutchProgressEvent {
            operation: GutchOperation::Fetch,
            phase: GutchProgressPhase::Done,
            message: "Target install content fetched".to_string(),
            current: required_bundle_ids.len(),
            total: required_bundle_ids.len().max(1),
            transferred_bytes: transfer_progress.transferred_bytes(),
            total_bytes: transfer_progress.total_bytes(),
            detail: Some(request.target_id.clone()),
            done: true,
        },
    );

    Ok(build_fetch_result(
        &prepared.repo_paths.repo_root,
        &request.target_id,
        required_bundle_ids.len(),
        downloaded_bundle_count,
    ))
}

fn download_and_apply_unified_target_install(
    request: TargetInstallDownloadAndApplyRequest,
) -> Result<TargetInstallDownloadAndApplyResult> {
    request.fetch_request.control.checkpoint()?;
    let progress = request.fetch_request.control.progress().clone();
    let download_client = build_download_client()?;
    let transfer_progress = Arc::new(FetchTransferProgress::default());
    let prepared = prepare_target_install_fetch(
        &request.fetch_request,
        &download_client,
        &progress,
        &transfer_progress,
    )?;
    let resolved_source = resolve_install_source(&request, &prepared.repo_paths)?;
    let verified_source = resolve_verified_source_install(
        resolved_source
            .source_manifest
            .as_ref()
            .zip(resolved_source.source_install_dir.as_deref())
            .map(
                |(source_manifest, source_install_dir)| TargetInstallApplySource {
                    target_manifest: source_manifest,
                    install_dir: source_install_dir,
                },
            ),
        &request.runtime_hints,
        &request.fetch_request.control,
    )?;

    let pipeline_plan = build_install_pipeline_plan(
        &prepared.target_manifest,
        &prepared.chunk_index,
        &prepared.repo_paths,
        &prepared.existing_bundle_ids,
        verified_source,
    )?;
    let pending_bundle_downloads = select_pending_bundle_downloads(
        &prepared.pending_bundle_downloads,
        &pipeline_plan.required_bundle_ids,
    );

    let fetch_result = build_fetch_result(
        &prepared.repo_paths.repo_root,
        &request.fetch_request.target_id,
        pipeline_plan.required_bundle_count,
        pending_bundle_downloads.len(),
    );
    let apply_result = apply_install_pipeline(
        InstallPipelineApplyRequest {
            repo_root: &prepared.repo_paths.repo_root,
            compression_profile: prepared.compression_profile.clone(),
            target_manifest: &prepared.target_manifest,
            install_dir: &request.install_dir,
            staging_dir: request.staging_dir.as_deref(),
            source_install_dir: resolved_source.source_install_dir,
            runtime_hints: &request.runtime_hints,
            existing_bundle_ids: &prepared.existing_bundle_ids,
            pending_bundle_downloads: &pending_bundle_downloads,
            control: &request.fetch_request.control,
            download_client: Some(&download_client),
            transfer_progress: Some(&transfer_progress),
            session_id: request.fetch_request.session_id.as_deref(),
        },
        pipeline_plan,
    )?;

    Ok(TargetInstallDownloadAndApplyResult {
        fetch_result,
        apply_result,
    })
}

fn prepare_target_install_fetch(
    request: &TargetInstallFetchRequest,
    download_client: &reqwest::blocking::Client,
    progress: &Option<Arc<dyn GutchProgressSink>>,
    transfer_progress: &Arc<FetchTransferProgress>,
) -> Result<PreparedTargetInstallFetch> {
    request.control.checkpoint()?;
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
            detail: Some(request.target_id.clone()),
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
        let repo_manifest_bytes = download_url(
            download_client,
            repo_metadata_url,
            request.session_id.as_deref(),
            &request.control,
            transfer_progress,
            GutchProgressPhase::RepoMetadata,
            "Downloading repo metadata",
            TransferPhaseCounters {
                current: 0,
                total: 1,
            },
            None,
        )?;
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
            phase: GutchProgressPhase::TargetMetadata,
            message: "Downloading target metadata".to_string(),
            current: 0,
            total: 1,
            transferred_bytes: 0,
            total_bytes: 0,
            detail: Some(request.target_id.clone()),
            done: false,
        },
    );
    let target_manifest_bytes = download_url(
        download_client,
        &request.metadata_url,
        request.session_id.as_deref(),
        &request.control,
        transfer_progress,
        GutchProgressPhase::TargetMetadata,
        "Downloading target metadata",
        TransferPhaseCounters {
            current: 0,
            total: 1,
        },
        Some(request.target_id.as_str()),
    )?;
    let target_metadata =
        target_manifest_from_download(&request.target_id, &target_manifest_bytes)?;
    crate::repo::write_target_metadata_only(&repo_paths, &target_metadata)?;

    if let Some(product_metadata_url) = request.product_metadata_url.as_deref() {
        let product_metadata_bytes = download_url(
            download_client,
            product_metadata_url,
            request.session_id.as_deref(),
            &request.control,
            transfer_progress,
            GutchProgressPhase::TargetMetadata,
            "Downloading target product metadata",
            TransferPhaseCounters {
                current: 0,
                total: 1,
            },
            Some(request.target_id.as_str()),
        )?;
        let product_metadata: crate::manifest::TargetProductMetadata =
            crate::helpers::from_json_bytes(&product_metadata_bytes)?;
        product_metadata.validate_structure()?;
        crate::repo::write_target_product_metadata(
            &repo_paths,
            &request.target_id,
            Some(&product_metadata),
        )?;
    } else {
        crate::repo::write_target_product_metadata(&repo_paths, &request.target_id, None)?;
    }

    let blueprint_bytes = download_url(
        download_client,
        &request.blueprint_url,
        request.session_id.as_deref(),
        &request.control,
        transfer_progress,
        GutchProgressPhase::TargetMetadata,
        "Downloading target blueprint",
        TransferPhaseCounters {
            current: 0,
            total: 1,
        },
        Some(request.target_id.as_str()),
    )?;
    crate::helpers::write(
        &crate::target_blueprint_path(&repo_paths, &request.target_id),
        &blueprint_bytes,
    )?;
    let target_manifest = crate::load_target_manifest(&repo_paths, &request.target_id)?;

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
        let chunk_index_bytes = download_url(
            download_client,
            chunk_index_url,
            request.session_id.as_deref(),
            &request.control,
            transfer_progress,
            GutchProgressPhase::ChunkIndex,
            "Downloading chunk index",
            TransferPhaseCounters {
                current: 0,
                total: 1,
            },
            None,
        )?;
        let chunk_index: crate::manifest::ChunkIndex =
            crate::helpers::from_json_bytes(&chunk_index_bytes)?;
        crate::write_chunk_index(&repo_paths, &chunk_index)?;
    }

    request.control.checkpoint()?;
    let chunk_index = crate::load_chunk_index(&repo_paths)?;
    let (_, repo_manifest) = crate::load_gutch_repo_manifest(&repo_paths.repo_root)?;
    let compression_profile = repo_manifest.compression_profile();
    let required_bundle_ids = crate::repo::required_bundle_ids(&target_manifest, &chunk_index);
    let existing_bundle_ids = required_bundle_ids
        .iter()
        .filter(|bundle_id| crate::bundle_path(&repo_paths, bundle_id).is_file())
        .cloned()
        .collect::<HashSet<_>>();
    let bundle_urls = fetch_bundle_urls(request, &required_bundle_ids, &existing_bundle_ids)?;
    let pending_bundle_downloads = build_pending_bundle_downloads(
        &required_bundle_ids,
        &existing_bundle_ids,
        &bundle_urls,
        &repo_paths,
    )?;

    Ok(PreparedTargetInstallFetch {
        repo_paths,
        compression_profile,
        target_manifest,
        chunk_index,
        required_bundle_ids,
        existing_bundle_ids,
        pending_bundle_downloads,
    })
}

fn build_pending_bundle_downloads(
    required_bundle_ids: &HashSet<String>,
    existing_bundle_ids: &HashSet<String>,
    bundle_urls: &HashMap<String, String>,
    repo_paths: &crate::GutchRepoPaths,
) -> Result<Vec<PendingBundleDownload>> {
    let total_missing_bundle_count = required_bundle_ids
        .len()
        .saturating_sub(existing_bundle_ids.len());
    required_bundle_ids
        .iter()
        .filter(|bundle_id| !existing_bundle_ids.contains(*bundle_id))
        .enumerate()
        .map(|(ordinal, bundle_id)| {
            let url = bundle_urls.get(bundle_id).ok_or_else(|| {
                Error::manifest(format!(
                    "missing download URL for required bundle '{}'",
                    bundle_id
                ))
            })?;
            Ok(PendingBundleDownload {
                bundle_id: bundle_id.clone(),
                url: url.clone(),
                bundle_path: crate::bundle_path(repo_paths, bundle_id),
                ordinal,
                total_missing_bundle_count,
            })
        })
        .collect()
}

fn resolve_install_source(
    request: &TargetInstallDownloadAndApplyRequest,
    repo_paths: &crate::GutchRepoPaths,
) -> Result<ResolvedInstallSource> {
    let source_manifest = match request.source_target_id.as_ref() {
        Some(source_target_id) => Some(crate::load_target_manifest(repo_paths, source_target_id)?),
        None => None,
    };
    let source_install_dir = source_manifest.as_ref().map(|_| {
        request
            .source_dir
            .clone()
            .unwrap_or_else(|| request.install_dir.clone())
    });
    Ok(ResolvedInstallSource {
        source_manifest,
        source_install_dir,
    })
}

fn select_pending_bundle_downloads(
    pending_bundle_downloads: &[PendingBundleDownload],
    required_bundle_ids: &HashSet<String>,
) -> Vec<PendingBundleDownload> {
    pending_bundle_downloads
        .iter()
        .filter(|bundle_download| required_bundle_ids.contains(&bundle_download.bundle_id))
        .cloned()
        .collect()
}

fn build_fetch_result(
    repo_root: &std::path::Path,
    target_id: &str,
    required_bundle_count: usize,
    downloaded_bundle_count: usize,
) -> TargetInstallFetchResult {
    TargetInstallFetchResult {
        repo_path: repo_root.to_path_buf(),
        target_id: target_id.to_string(),
        required_bundle_count,
        downloaded_bundle_count,
        skipped_existing_bundle_count: required_bundle_count
            .saturating_sub(downloaded_bundle_count),
    }
}

fn download_pending_bundle_files(
    request: &TargetInstallFetchRequest,
    download_client: &reqwest::blocking::Client,
    progress: &Option<Arc<dyn GutchProgressSink>>,
    transfer_progress: &Arc<FetchTransferProgress>,
    pending_bundle_downloads: &[PendingBundleDownload],
) -> Result<usize> {
    if pending_bundle_downloads.is_empty() {
        return Ok(0);
    }

    install_in_thread_pool(
        bundle_download_worker_count(pending_bundle_downloads.len()),
        || {
            pending_bundle_downloads
                .par_iter()
                .map(|bundle_download| {
                    request.control.checkpoint()?;
                    emit_progress(
                        progress,
                        GutchProgressEvent {
                            operation: GutchOperation::Fetch,
                            phase: GutchProgressPhase::Bundle,
                            message: format!(
                                "Downloading bundles ({}/{})",
                                bundle_download.ordinal + 1,
                                bundle_download.total_missing_bundle_count.max(1)
                            ),
                            current: transfer_progress.completed_bundle_count(),
                            total: bundle_download.total_missing_bundle_count.max(1),
                            transferred_bytes: transfer_progress.transferred_bytes(),
                            total_bytes: transfer_progress.total_bytes(),
                            detail: Some(bundle_download.bundle_id.clone()),
                            done: false,
                        },
                    );
                    let completed_bundle_count = download_pending_bundle(
                        download_client,
                        bundle_download,
                        request.session_id.as_deref(),
                        &request.control,
                        transfer_progress,
                    )?;
                    emit_progress(
                        progress,
                        GutchProgressEvent {
                            operation: GutchOperation::Fetch,
                            phase: GutchProgressPhase::Bundle,
                            message: format!(
                                "Downloaded bundles ({}/{})",
                                completed_bundle_count,
                                bundle_download.total_missing_bundle_count.max(1)
                            ),
                            current: completed_bundle_count,
                            total: bundle_download.total_missing_bundle_count.max(1),
                            transferred_bytes: transfer_progress.transferred_bytes(),
                            total_bytes: transfer_progress.total_bytes(),
                            detail: Some(bundle_download.bundle_id.clone()),
                            done: false,
                        },
                    );
                    Ok::<(), Error>(())
                })
                .collect::<Vec<_>>()
        },
    )?
    .into_iter()
    .collect::<Result<Vec<_>>>()?;
    Ok(pending_bundle_downloads.len())
}

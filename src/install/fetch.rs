use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde::Deserialize;

use super::apply::apply_target_install_from_repo;
use super::types::{
    GutchBundleDownload, TargetInstallApplyFromRepoRequest, TargetInstallDownloadAndApplyRequest,
    TargetInstallDownloadAndApplyResult, TargetInstallFetchRequest, TargetInstallFetchResult,
};
use crate::manifest::gutch_repo::GutchRepoManifest;
use crate::manifest::target::TargetManifest;
use crate::progress::{
    GutchOperation, GutchProgressEvent, GutchProgressPhase, GutchProgressSink, emit_progress,
};
use crate::{Error, Result};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GutchBundleUrlsResponse {
    #[serde(default)]
    bundles: Vec<GutchBundleDownload>,
}

pub fn download_and_apply_target_install(
    request: TargetInstallDownloadAndApplyRequest,
) -> Result<TargetInstallDownloadAndApplyResult> {
    let fetch_result = fetch_target_install_content(&request.fetch_request)?;
    let apply_result = apply_target_install_from_repo(TargetInstallApplyFromRepoRequest {
        repo_path: request.fetch_request.repo_path.clone(),
        target_id: request.fetch_request.target_id.clone(),
        install_dir: request.install_dir,
        staging_dir: request.staging_dir,
        source_target_id: request.source_target_id,
        source_dir: request.source_dir,
        runtime_hints: request.runtime_hints,
    })?;

    Ok(TargetInstallDownloadAndApplyResult {
        fetch_result,
        apply_result,
    })
}

pub fn fetch_target_install_content(
    request: &TargetInstallFetchRequest,
) -> Result<TargetInstallFetchResult> {
    let progress = request.progress.clone();
    emit_progress(
        &progress,
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

    if let Some(repo_manifest_url) = request.repo_manifest_url.as_deref() {
        emit_progress(
            &progress,
            GutchProgressEvent {
                operation: GutchOperation::Fetch,
                phase: GutchProgressPhase::RepoManifest,
                message: "Downloading repo manifest".to_string(),
                current: 0,
                total: 1,
                transferred_bytes: 0,
                total_bytes: 0,
                detail: None,
                done: false,
            },
        );
        let repo_manifest_bytes = download_url(
            repo_manifest_url,
            request.session_id.as_deref(),
            &progress,
            GutchProgressPhase::RepoManifest,
            None,
        )?;
        let repo_manifest: GutchRepoManifest =
            crate::helpers::from_json_bytes(&repo_manifest_bytes)?;
        write_repo_manifest(&repo_paths, &repo_manifest)?;
    } else if !repo_paths.repo_manifest_path.exists() {
        let default_manifest = GutchRepoManifest::new("download".to_string());
        write_repo_manifest(&repo_paths, &default_manifest)?;
    }

    emit_progress(
        &progress,
        GutchProgressEvent {
            operation: GutchOperation::Fetch,
            phase: GutchProgressPhase::TargetManifest,
            message: "Downloading target manifest".to_string(),
            current: 0,
            total: 1,
            transferred_bytes: 0,
            total_bytes: 0,
            detail: Some(request.target_id.clone()),
            done: false,
        },
    );
    let target_manifest_bytes = download_url(
        &request.manifest_url,
        request.session_id.as_deref(),
        &progress,
        GutchProgressPhase::TargetManifest,
        Some(request.target_id.as_str()),
    )?;
    let target_manifest =
        target_manifest_from_download(&repo_paths, &request.target_id, &target_manifest_bytes)?;
    crate::write_target_manifest_artifacts(&repo_paths, &target_manifest)?;

    if let Some(chunk_index_url) = request.chunk_index_url.as_deref() {
        emit_progress(
            &progress,
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
            chunk_index_url,
            request.session_id.as_deref(),
            &progress,
            GutchProgressPhase::ChunkIndex,
            None,
        )?;
        let chunk_index: crate::manifest::ChunkIndex =
            crate::helpers::from_json_bytes(&chunk_index_bytes)?;
        crate::write_chunk_index(&repo_paths, &chunk_index)?;
    }

    let chunk_index = crate::load_chunk_index(&repo_paths)?;
    let required_bundle_ids = crate::repo::required_bundle_ids(&target_manifest, &chunk_index);
    let existing_bundle_ids = required_bundle_ids
        .iter()
        .filter(|bundle_id| crate::bundle_path(&repo_paths, bundle_id).is_file())
        .cloned()
        .collect::<HashSet<_>>();

    let bundle_urls = fetch_bundle_urls(request, &required_bundle_ids, &existing_bundle_ids)?;
    let mut downloaded_bundle_count = 0usize;
    let skipped_existing_bundle_count = existing_bundle_ids.len();

    for (index, bundle_id) in required_bundle_ids.iter().enumerate() {
        if existing_bundle_ids.contains(bundle_id) {
            continue;
        }
        let url = bundle_urls.get(bundle_id).ok_or_else(|| {
            Error::manifest(format!(
                "missing download URL for required bundle '{}'",
                bundle_id
            ))
        })?;
        let detail = Some(bundle_id.clone());
        emit_progress(
            &progress,
            GutchProgressEvent {
                operation: GutchOperation::Fetch,
                phase: GutchProgressPhase::Bundle,
                message: format!(
                    "Downloading bundle {}/{}",
                    downloaded_bundle_count + 1,
                    required_bundle_ids
                        .len()
                        .saturating_sub(skipped_existing_bundle_count)
                ),
                current: index,
                total: required_bundle_ids.len(),
                transferred_bytes: 0,
                total_bytes: 0,
                detail: detail.clone(),
                done: false,
            },
        );
        let bundle_bytes = download_url(
            url,
            request.session_id.as_deref(),
            &progress,
            GutchProgressPhase::Bundle,
            detail.as_deref(),
        )?;
        let bundle_path = crate::bundle_path(&repo_paths, bundle_id);
        if let Some(parent) = bundle_path.parent() {
            crate::helpers::create_dir_all(parent)?;
        }
        crate::helpers::write(&bundle_path, &bundle_bytes)?;
        downloaded_bundle_count += 1;
    }

    emit_progress(
        &progress,
        GutchProgressEvent {
            operation: GutchOperation::Fetch,
            phase: GutchProgressPhase::Done,
            message: "Target install content fetched".to_string(),
            current: required_bundle_ids.len(),
            total: required_bundle_ids.len().max(1),
            transferred_bytes: 0,
            total_bytes: 0,
            detail: Some(request.target_id.clone()),
            done: true,
        },
    );

    Ok(TargetInstallFetchResult {
        repo_path: repo_paths.repo_root,
        target_id: request.target_id.clone(),
        required_bundle_count: required_bundle_ids.len(),
        downloaded_bundle_count,
        skipped_existing_bundle_count,
    })
}

fn normalize_bundle_downloads(bundles: &[GutchBundleDownload]) -> Result<HashMap<String, String>> {
    let mut normalized = HashMap::with_capacity(bundles.len());
    for bundle in bundles {
        let bundle_id = non_empty_trimmed(&bundle.bundle_id, "bundle id")?;
        let url = non_empty_trimmed(&bundle.url, "bundle URL")?;
        normalized.insert(bundle_id.to_string(), url.to_string());
    }
    Ok(normalized)
}

fn download_url(
    url: &str,
    session_id: Option<&str>,
    progress: &Option<Arc<dyn GutchProgressSink>>,
    phase: GutchProgressPhase,
    detail: Option<&str>,
) -> Result<Vec<u8>> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("Gutch CLI")
        .build()
        .map_err(|err| Error::manifest(format!("failed to create download client: {}", err)))?;
    let mut request = client.get(url);
    if should_attach_bearer_session(url, session_id) {
        request = request.header(
            "Authorization",
            format!("Bearer {}", session_id.expect("session already checked")),
        );
    }
    let response = request
        .send()
        .map_err(|err| Error::manifest(format!("download request failed: {}", err)))?;
    if !response.status().is_success() {
        return Err(Error::manifest(format!(
            "download request failed with status {}",
            response.status()
        )));
    }
    let bytes = response
        .bytes()
        .map_err(|err| Error::manifest(format!("failed to read download response: {}", err)))?;
    emit_progress(
        progress,
        GutchProgressEvent {
            operation: GutchOperation::Fetch,
            phase,
            message: "Downloaded content".to_string(),
            current: 1,
            total: 1,
            transferred_bytes: bytes.len() as u64,
            total_bytes: bytes.len() as u64,
            detail: detail.map(str::to_string),
            done: false,
        },
    );
    Ok(bytes.to_vec())
}

fn should_attach_bearer_session(url: &str, session_id: Option<&str>) -> bool {
    session_id.is_some() && !url.contains("X-Amz-Algorithm=") && !url.contains("sig=")
}

fn download_json_bytes(url: &str, session_id: Option<&str>) -> Result<Vec<u8>> {
    download_url(url, session_id, &None, GutchProgressPhase::Json, None)
}

fn target_manifest_from_download(
    repo_paths: &crate::GutchRepoPaths,
    target_id: &str,
    manifest_bytes: &[u8],
) -> Result<TargetManifest> {
    let manifest_json = std::str::from_utf8(manifest_bytes)
        .map_err(|err| Error::manifest(format!("target manifest is not valid UTF-8: {}", err)))?;
    let manifest_value: serde_json::Value = serde_json::from_str(manifest_json)?;
    let mut manifest = if manifest_value.get("files").is_some() {
        serde_json::from_value::<TargetManifest>(manifest_value)?
    } else {
        let shell: crate::manifest::binary::TargetManifestShell =
            serde_json::from_str(manifest_json)?;
        crate::manifest::binary::target_manifest_from_shell(
            shell,
            target_id.to_string(),
            Vec::new(),
        )
    };
    if manifest.target_id.trim().is_empty() {
        manifest.target_id = target_id.to_string();
    }
    if manifest.target_id != target_id {
        manifest.target_id = target_id.to_string();
    }
    let _ = repo_paths;
    Ok(manifest)
}

fn non_empty_trimmed<'a>(value: &'a str, label: &str) -> Result<&'a str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(Error::manifest(format!("{label} is required")));
    }
    Ok(trimmed)
}

fn fetch_bundle_urls(
    request: &TargetInstallFetchRequest,
    required_bundle_ids: &HashSet<String>,
    existing_bundle_ids: &HashSet<String>,
) -> Result<HashMap<String, String>> {
    let mut bundle_urls = normalize_bundle_downloads(&request.bundles)?;
    let unresolved_bundle_ids = required_bundle_ids
        .iter()
        .filter(|bundle_id| {
            !existing_bundle_ids.contains(*bundle_id)
                && !bundle_urls.contains_key(bundle_id.as_str())
        })
        .cloned()
        .collect::<Vec<_>>();
    if unresolved_bundle_ids.is_empty() {
        return Ok(bundle_urls);
    }

    let endpoint = request
        .bundle_downloads_endpoint
        .as_deref()
        .ok_or_else(|| {
            Error::manifest(
                "bundle-downloads-endpoint is required when direct bundle URLs are not provided",
            )
        })?;
    let ids = unresolved_bundle_ids.join(",");
    let separator = if endpoint.contains('?') { "&" } else { "?" };
    let response_bytes = download_json_bytes(
        &format!("{endpoint}{separator}ids={ids}"),
        request.session_id.as_deref(),
    )?;
    let response: GutchBundleUrlsResponse = crate::helpers::from_json_bytes(&response_bytes)?;
    for bundle in response.bundles {
        let bundle_id = non_empty_trimmed(&bundle.bundle_id, "bundle id")?;
        let url = non_empty_trimmed(&bundle.url, "bundle URL")?;
        bundle_urls.insert(bundle_id.to_string(), url.to_string());
    }

    let missing = unresolved_bundle_ids
        .into_iter()
        .filter(|bundle_id| !bundle_urls.contains_key(bundle_id))
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(Error::manifest(format!(
            "missing bundle download URLs for: {}",
            missing.join(", ")
        )));
    }
    Ok(bundle_urls)
}

fn write_repo_manifest(
    repo_paths: &crate::GutchRepoPaths,
    repo_manifest: &GutchRepoManifest,
) -> Result<()> {
    crate::write_gutch_repo_manifest(repo_paths, repo_manifest)
}

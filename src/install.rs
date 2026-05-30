use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::manifest::{self, target::TargetManifest};
use crate::reconstruct::{
    ReconstructTimings, VerifiedSourceInstall,
    reconstruct_from_bundle_paths_and_verified_source_with_timings_and_hints,
};
use crate::runtime::RuntimeResourceHints;

use crate::verify::{
    VerifyTimings, classify_directory_with_progress, verify_directory_with_progress,
};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};

fn derive_chunk_count(manifest: &TargetManifest) -> usize {
    crate::repo::required_chunk_hashes(manifest).len()
}

#[derive(Debug, Clone)]
pub struct TargetInstallApplySource<'a> {
    pub target_manifest: &'a TargetManifest,
    pub install_dir: &'a Path,
}

#[derive(Debug, Clone)]
pub struct TargetInstallApplyResult {
    pub target_id: String,
    pub install_dir: PathBuf,
    pub staging_dir: PathBuf,
    pub backup_dir: Option<PathBuf>,
    pub reused_source_chunk_count: usize,
    pub required_chunk_count: usize,
    pub required_bundle_count: usize,
    pub reconstruct_timings: ReconstructTimings,
    pub verify_timings: VerifyTimings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GutchBundleDownload {
    pub bundle_id: String,
    pub url: String,
}

#[derive(Debug, Clone)]
pub struct TargetInstallFetchRequest {
    pub repo_path: PathBuf,
    pub target_id: String,
    pub manifest_url: String,
    pub bundle_urls_endpoint: Option<String>,
    pub session_id: Option<String>,
    pub bundles: Vec<GutchBundleDownload>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetInstallFetchResult {
    pub target_id: String,
    pub repo_path: PathBuf,
    pub required_bundle_count: usize,
    pub downloaded_bundle_count: usize,
    pub skipped_existing_bundle_count: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GutchBundleUrlsResponse {
    #[serde(default)]
    bundles: Vec<GutchBundleDownload>,
}

pub fn apply_target_install_with_timings(
    target_manifest: &TargetManifest,
    bundle_paths: &[PathBuf],
    install_dir: &Path,
    staging_dir: &Path,
    source: Option<TargetInstallApplySource<'_>>,
) -> Result<TargetInstallApplyResult> {
    apply_target_install_with_timings_and_hints(
        target_manifest,
        bundle_paths,
        install_dir,
        staging_dir,
        source,
        &RuntimeResourceHints::default(),
    )
}

pub fn apply_target_install_with_timings_and_hints(
    target_manifest: &TargetManifest,
    bundle_paths: &[PathBuf],
    install_dir: &Path,
    staging_dir: &Path,
    source: Option<TargetInstallApplySource<'_>>,
    runtime_hints: &RuntimeResourceHints,
) -> Result<TargetInstallApplyResult> {
    if install_dir == staging_dir {
        return Err(Error::PathError {
            path: install_dir.to_path_buf(),
            reason: "install and staging directories must be different".to_string(),
        });
    }

    let source_manifest = source.as_ref().map(|value| value.target_manifest);
    let verified_source = match source {
        Some(source) => {
            let (source_classification, _) = classify_directory_with_progress(
                source.target_manifest,
                source.install_dir,
                &RuntimeResourceHints::default(),
            )?;
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
                return Err(Error::verify(format!(
                    "source install did not verify: {}",
                    source_errors
                )));
            }

            Some(VerifiedSourceInstall {
                target_manifest: source.target_manifest,
                install_dir: source.install_dir,
            })
        }
        None => None,
    };

    if staging_dir.exists() {
        std::fs::remove_dir_all(staging_dir)?;
    }
    if let Some(parent) = staging_dir.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let reused_source_chunk_count = verified_source
        .as_ref()
        .map(|source| count_reusable_source_chunks(target_manifest, source.target_manifest))
        .unwrap_or(0);

    let (_, reconstruct_timings) =
        reconstruct_from_bundle_paths_and_verified_source_with_timings_and_hints(
            target_manifest,
            bundle_paths,
            verified_source,
            staging_dir,
            runtime_hints,
        )?;

    let (verify_errors, verify_timings) =
        verify_directory_with_progress(target_manifest, staging_dir, runtime_hints)?;
    if !verify_errors.is_empty() {
        return Err(Error::verify(format!(
            "staged install did not verify: {}",
            verify_errors.join("; ")
        )));
    }

    let backup_dir = promote_staging_install(install_dir, staging_dir, target_manifest)?;
    let required_bundle_count =
        count_required_bundle_paths(target_manifest, bundle_paths, source_manifest)?;

    Ok(TargetInstallApplyResult {
        target_id: target_manifest.target_id.clone(),
        install_dir: install_dir.to_path_buf(),
        staging_dir: staging_dir.to_path_buf(),
        backup_dir,
        reused_source_chunk_count,
        required_chunk_count: derive_chunk_count(target_manifest),
        required_bundle_count,
        reconstruct_timings,
        verify_timings,
    })
}

pub fn fetch_target_install_content(
    request: &TargetInstallFetchRequest,
) -> Result<TargetInstallFetchResult> {
    let repo_paths = crate::GutchRepoPaths::from_repo_root(&request.repo_path);
    std::fs::create_dir_all(&repo_paths.targets_root)?;
    std::fs::create_dir_all(&repo_paths.bundles_root)?;

    let manifest_bytes = download_url(request.manifest_url.trim())?;
    let manifest: serde_json::Value = serde_json::from_slice(&manifest_bytes).map_err(|err| {
        Error::manifest(format!(
            "downloaded target manifest is invalid JSON: {}",
            err
        ))
    })?;
    write_repo_manifest(&request.repo_path, &manifest)?;

    let bundle_ids_from_chunk_index: Vec<String> = {
        let chunk_index = crate::load_chunk_index(&repo_paths).ok();

        if let Some(index) = chunk_index {
            let mut target: TargetManifest = serde_json::from_slice(&manifest_bytes)
                .map_err(|err| Error::manifest(format!("invalid target manifest: {}", err)))?;
            if target.target_id.trim().is_empty() {
                target.target_id = request.target_id.clone();
            }
            crate::repo::required_bundle_ids(&target, &index)
                .into_iter()
                .collect()
        } else {
            Vec::new()
        }
    };
    let mut target_manifest: TargetManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|err| Error::manifest(format!("invalid target manifest: {}", err)))?;
    if target_manifest.target_id.trim().is_empty() {
        target_manifest.target_id = request.target_id.clone();
    }
    crate::write_target_manifest_artifacts(&repo_paths, &target_manifest)?;
    let bundles = if request.bundles.is_empty() && !bundle_ids_from_chunk_index.is_empty() {
        fetch_bundle_urls(request, &bundle_ids_from_chunk_index)?
    } else {
        request.bundles.clone()
    };

    let mut downloaded_bundle_count = 0usize;
    let mut skipped_existing_bundle_count = 0usize;
    for bundle in &bundles {
        let bundle_id = bundle.bundle_id.trim();
        if bundle_id.is_empty() {
            continue;
        }

        let bundle_path = crate::bundle_path(&repo_paths, bundle_id);
        if bundle_path.exists() {
            skipped_existing_bundle_count += 1;
            continue;
        }

        let bundle_bytes = download_url(bundle.url.trim())?;
        std::fs::write(&bundle_path, bundle_bytes)?;
        downloaded_bundle_count += 1;
    }

    Ok(TargetInstallFetchResult {
        target_id: request.target_id.clone(),
        repo_path: request.repo_path.clone(),
        required_bundle_count: bundle_ids_from_chunk_index.len(),
        downloaded_bundle_count,
        skipped_existing_bundle_count,
    })
}

fn count_reusable_source_chunks(
    target_manifest: &TargetManifest,
    source_manifest: &TargetManifest,
) -> usize {
    let target_chunks = crate::repo::required_chunk_hashes(target_manifest);
    let source_chunks = crate::repo::required_chunk_hashes(source_manifest);
    source_chunks
        .iter()
        .filter(|chunk_hash| target_chunks.contains(chunk_hash.as_str()))
        .count()
}

fn count_required_bundle_paths(
    target_manifest: &TargetManifest,
    bundle_paths: &[PathBuf],
    source_manifest: Option<&TargetManifest>,
) -> Result<usize> {
    let mut needed_chunks = crate::repo::required_chunk_hashes(target_manifest);
    if let Some(source_manifest) = source_manifest {
        for chunk_hash in crate::repo::required_chunk_hashes(source_manifest) {
            needed_chunks.remove(&chunk_hash);
        }
    }
    if needed_chunks.is_empty() {
        return Ok(0);
    }

    let mut covered_chunks = HashSet::new();
    let mut required_bundle_count = 0usize;

    for bundle_path in bundle_paths {
        let file = std::fs::File::open(bundle_path)?;
        let reader = crate::bundle::BundleReader::open(file)?;
        let mut matched_bundle = false;

        for entry in reader.entries() {
            if needed_chunks.contains(&entry.chunk_hash)
                && covered_chunks.insert(entry.chunk_hash.clone())
            {
                matched_bundle = true;
            }
        }

        if matched_bundle {
            required_bundle_count += 1;
            if covered_chunks.len() == needed_chunks.len() {
                break;
            }
        }
    }

    Ok(required_bundle_count)
}

fn promote_staging_install(
    install_dir: &Path,
    staging_dir: &Path,
    target_manifest: &TargetManifest,
) -> Result<Option<PathBuf>> {
    if let Some(parent) = install_dir.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let backup_dir = if install_dir.exists() {
        let backup_dir = backup_dir_for_install(install_dir, &target_manifest.target_id);
        if backup_dir.exists() {
            std::fs::remove_dir_all(&backup_dir)?;
        }
        std::fs::rename(install_dir, &backup_dir)?;
        Some(backup_dir)
    } else {
        None
    };

    if let Err(err) = std::fs::rename(staging_dir, install_dir) {
        if let Some(backup_dir) = &backup_dir {
            let _ = std::fs::rename(backup_dir, install_dir);
        }
        return Err(Error::Io(err));
    }

    if let Some(backup_dir) = &backup_dir {
        std::fs::remove_dir_all(backup_dir)?;
    }

    Ok(backup_dir)
}

fn backup_dir_for_install(install_dir: &Path, target_id: &str) -> PathBuf {
    let install_name = install_dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("install");
    let backup_name = format!(
        ".gutch-backup-{}-{}-{}",
        install_name,
        target_id,
        std::process::id()
    );
    install_dir.with_file_name(backup_name)
}

fn download_url(url: &str) -> Result<Vec<u8>> {
    if url.is_empty() {
        return Err(Error::bundle("download URL is required"));
    }

    let response = reqwest::blocking::get(url)
        .map_err(|err| Error::bundle(format!("failed to download Gutch content: {}", err)))?;
    let status = response.status();
    if !status.is_success() {
        return Err(Error::bundle(format!(
            "Gutch content download failed with status {}.",
            status
        )));
    }

    response
        .bytes()
        .map(|bytes| bytes.to_vec())
        .map_err(|err| Error::bundle(format!("failed to read Gutch download body: {}", err)))
}

fn fetch_bundle_urls(
    request: &TargetInstallFetchRequest,
    bundle_ids: &[String],
) -> Result<Vec<GutchBundleDownload>> {
    let endpoint = request
        .bundle_urls_endpoint
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Error::bundle("bundle URL endpoint is required for Gutch install"))?;
    let separator = if endpoint.contains('?') { '&' } else { '?' };
    let url = format!("{}{}ids={}", endpoint, separator, bundle_ids.join(","));
    let mut builder = reqwest::blocking::Client::new().get(url);
    if let Some(session_id) = request.session_id.as_deref().map(str::trim) {
        if !session_id.is_empty() {
            builder = builder.header("Authorization", format!("Bearer {}", session_id));
        }
    }

    let response = builder
        .send()
        .map_err(|err| Error::bundle(format!("failed to request Gutch bundle URLs: {}", err)))?;
    let status = response.status();
    if !status.is_success() {
        return Err(Error::bundle(format!(
            "Gutch bundle URL request failed with status {}.",
            status
        )));
    }

    response
        .json::<GutchBundleUrlsResponse>()
        .map(|body| body.bundles)
        .map_err(|err| Error::bundle(format!("failed to parse Gutch bundle URLs: {}", err)))
}

fn write_repo_manifest(repo_path: &Path, target_manifest: &serde_json::Value) -> Result<()> {
    let existing_manifest_path = repo_path.join(manifest::CONTENT_REPO_MANIFEST_FILE);
    let gutch_repo_id = if existing_manifest_path.exists() {
        let json = std::fs::read_to_string(&existing_manifest_path)?;
        let existing_manifest: crate::manifest::gutch_repo::GutchRepoManifest =
            serde_json::from_str(&json)?;
        existing_manifest.gutch_repo_id
    } else {
        repo_path
            .file_name()
            .and_then(|value| value.to_str())
            .filter(|value| !value.is_empty())
            .unwrap_or("base")
            .to_string()
    };
    let mut repo_manifest = crate::manifest::gutch_repo::GutchRepoManifest::new(gutch_repo_id);
    if let Some(inserted_at) = target_manifest
        .get("inserted_at")
        .or_else(|| target_manifest.get("created_at"))
        .and_then(serde_json::Value::as_str)
    {
        repo_manifest.created_at = inserted_at.to_string();
    }

    let repo_json = serde_json::to_vec_pretty(&repo_manifest)?;
    std::fs::write(
        repo_path.join(manifest::CONTENT_REPO_MANIFEST_FILE),
        repo_json,
    )?;
    Ok(())
}

#[cfg(test)]
#[path = "install_tests.rs"]
mod tests;

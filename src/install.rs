use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::manifest::{self, target::TargetManifest};
use crate::reconstruct::{
    ReconstructTimings, VerifiedSourceInstall,
    reconstruct_from_bundle_paths_and_verified_source_with_timings_and_hints,
    reconstruct_from_bundle_paths_with_timings_and_hints,
};
use crate::runtime::RuntimeResourceHints;

use crate::verify::{
    RepairClassification, RepairIssue, VerifyTimings, classify_directory_with_progress,
    classify_directory_with_progress_and_mode, verify_directory_with_progress,
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

#[derive(Debug, Clone)]
pub struct TargetInstallApplyFromRepoRequest {
    pub repo_path: PathBuf,
    pub target_id: String,
    pub install_dir: PathBuf,
    pub staging_dir: Option<PathBuf>,
    pub source_target_id: Option<String>,
    pub source_dir: Option<PathBuf>,
    pub runtime_hints: RuntimeResourceHints,
}

#[derive(Debug, Clone)]
pub struct TargetInstallApplyFromRepoResult {
    pub repo_root: PathBuf,
    pub target_manifest: TargetManifest,
    pub apply_result: TargetInstallApplyResult,
    pub resolved_bundle_file_count: usize,
    pub source_install_dir: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct TargetInstallVerifyFromRepoRequest {
    pub repo_path: PathBuf,
    pub target_id: String,
    pub install_dir: PathBuf,
    pub verify_mode: crate::VerifyMode,
    pub runtime_hints: RuntimeResourceHints,
}

#[derive(Debug, Clone)]
pub struct TargetInstallVerifyFromRepoResult {
    pub repo_root: PathBuf,
    pub target_manifest: TargetManifest,
    pub classification: RepairClassification,
    pub verify_timings: VerifyTimings,
}

#[derive(Debug, Clone)]
pub struct TargetInstallRepairResult {
    pub target_id: String,
    pub install_dir: PathBuf,
    pub missing_file_count: usize,
    pub corrupt_file_count: usize,
    pub extra_file_count: usize,
    pub removed_extra_file_count: usize,
    pub restored_extra_file_count: usize,
    pub apply_result: Option<TargetInstallApplyResult>,
    pub verify_timings: VerifyTimings,
}

#[derive(Debug, Clone)]
pub struct TargetInstallRepairFromRepoRequest {
    pub repo_path: PathBuf,
    pub target_id: String,
    pub install_dir: PathBuf,
    pub staging_dir: Option<PathBuf>,
    pub remove_added_files: bool,
    pub runtime_hints: RuntimeResourceHints,
}

#[derive(Debug, Clone)]
pub struct TargetInstallRepairFromRepoResult {
    pub repo_root: PathBuf,
    pub target_manifest: TargetManifest,
    pub repair_result: TargetInstallRepairResult,
    pub resolved_bundle_file_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GutchBundleDownload {
    pub bundle_id: String,
    #[serde(alias = "downloadUrl")]
    pub url: String,
}

#[derive(Clone)]
pub struct TargetInstallFetchRequest {
    pub repo_path: PathBuf,
    pub target_id: String,
    pub manifest_url: String,
    pub chunk_index_url: Option<String>,
    pub bundle_downloads_endpoint: Option<String>,
    pub session_id: Option<String>,
    pub bundles: Vec<GutchBundleDownload>,
    pub progress: Option<std::sync::Arc<dyn crate::GutchProgressSink>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetInstallFetchResult {
    pub target_id: String,
    pub repo_path: PathBuf,
    pub required_bundle_count: usize,
    pub downloaded_bundle_count: usize,
    pub skipped_existing_bundle_count: usize,
}

#[derive(Clone)]
pub struct TargetInstallDownloadAndApplyRequest {
    pub fetch_request: TargetInstallFetchRequest,
    pub install_dir: PathBuf,
    pub staging_dir: Option<PathBuf>,
    pub source_target_id: Option<String>,
    pub source_dir: Option<PathBuf>,
    pub runtime_hints: RuntimeResourceHints,
}

#[derive(Debug, Clone)]
pub struct TargetInstallDownloadAndApplyResult {
    pub fetch_result: TargetInstallFetchResult,
    pub apply_result: TargetInstallApplyFromRepoResult,
}

pub fn default_target_install_staging_dir(install_dir: &Path, target_id: &str) -> PathBuf {
    let install_name = install_dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("install");
    install_dir.with_file_name(format!(".gutch-staging-{}-{}", install_name, target_id))
}

pub fn resolve_required_bundle_paths_from_repo(
    repo_paths: &crate::GutchRepoPaths,
    target_manifest: &TargetManifest,
) -> Result<Vec<PathBuf>> {
    let chunk_index = crate::load_chunk_index(repo_paths)?;
    let required_bundle_ids = crate::repo::required_bundle_ids(target_manifest, &chunk_index);
    let mut bundle_paths = Vec::with_capacity(required_bundle_ids.len());
    let mut missing_bundles = Vec::new();
    for bundle_id in &required_bundle_ids {
        let bundle_path = crate::bundle_path(repo_paths, bundle_id);
        if bundle_path.exists() {
            bundle_paths.push(bundle_path);
        } else {
            missing_bundles.push(bundle_id.clone());
        }
    }
    if !missing_bundles.is_empty() {
        return Err(Error::manifest(format!(
            "one or more required bundles are unavailable: {}",
            missing_bundles.join(", ")
        )));
    }
    Ok(bundle_paths)
}

pub fn verify_target_install_from_repo(
    request: TargetInstallVerifyFromRepoRequest,
) -> Result<TargetInstallVerifyFromRepoResult> {
    let (repo_paths, _) = crate::load_gutch_repo_manifest(&request.repo_path)?;
    let target_manifest = crate::load_target_manifest(&repo_paths, &request.target_id)?;
    let (classification, verify_timings) = classify_directory_with_progress_and_mode(
        &target_manifest,
        &request.install_dir,
        &request.runtime_hints,
        request.verify_mode,
    )?;

    Ok(TargetInstallVerifyFromRepoResult {
        repo_root: repo_paths.repo_root,
        target_manifest,
        classification,
        verify_timings,
    })
}

pub fn apply_target_install_from_repo(
    request: TargetInstallApplyFromRepoRequest,
) -> Result<TargetInstallApplyFromRepoResult> {
    let (repo_paths, _) = crate::load_gutch_repo_manifest(&request.repo_path)?;
    let target_manifest = crate::load_target_manifest(&repo_paths, &request.target_id)?;
    let staging_dir = request.staging_dir.unwrap_or_else(|| {
        default_target_install_staging_dir(&request.install_dir, &target_manifest.target_id)
    });
    let bundle_paths = resolve_required_bundle_paths_from_repo(&repo_paths, &target_manifest)?;

    let source_manifest = match request.source_target_id.as_ref() {
        Some(source_target_id) => Some(crate::load_target_manifest(&repo_paths, source_target_id)?),
        None => None,
    };
    let source_install_dir = source_manifest.as_ref().map(|_| {
        request
            .source_dir
            .clone()
            .unwrap_or_else(|| request.install_dir.clone())
    });
    let source = source_manifest
        .as_ref()
        .zip(source_install_dir.as_ref())
        .map(
            |(source_manifest, source_install_dir)| TargetInstallApplySource {
                target_manifest: source_manifest,
                install_dir: source_install_dir,
            },
        );

    let apply_result = apply_target_install_with_timings_and_hints(
        &target_manifest,
        &bundle_paths,
        &request.install_dir,
        &staging_dir,
        source,
        &request.runtime_hints,
    )?;

    Ok(TargetInstallApplyFromRepoResult {
        repo_root: repo_paths.repo_root,
        target_manifest,
        apply_result,
        resolved_bundle_file_count: bundle_paths.len(),
        source_install_dir,
    })
}

pub fn repair_target_install_from_repo(
    request: TargetInstallRepairFromRepoRequest,
) -> Result<TargetInstallRepairFromRepoResult> {
    let (repo_paths, _) = crate::load_gutch_repo_manifest(&request.repo_path)?;
    let target_manifest = crate::load_target_manifest(&repo_paths, &request.target_id)?;
    let bundle_paths = resolve_required_bundle_paths_from_repo(&repo_paths, &target_manifest)?;
    let staging_dir = request.staging_dir.unwrap_or_else(|| {
        default_target_install_staging_dir(&request.install_dir, &target_manifest.target_id)
    });
    let repair_result = repair_target_install_with_timings_and_hints(
        &target_manifest,
        &bundle_paths,
        &request.install_dir,
        &staging_dir,
        request.remove_added_files,
        &request.runtime_hints,
    )?;

    Ok(TargetInstallRepairFromRepoResult {
        repo_root: repo_paths.repo_root,
        target_manifest,
        repair_result,
        resolved_bundle_file_count: bundle_paths.len(),
    })
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
                runtime_hints,
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
        crate::helpers::remove_dir_all(staging_dir)?;
    }
    if let Some(parent) = staging_dir.parent() {
        crate::helpers::create_dir_all(parent)?;
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

pub fn repair_target_install_with_timings_and_hints(
    target_manifest: &TargetManifest,
    bundle_paths: &[PathBuf],
    install_dir: &Path,
    staging_dir: &Path,
    remove_added_files: bool,
    runtime_hints: &RuntimeResourceHints,
) -> Result<TargetInstallRepairResult> {
    let (classification, initial_verify_timings) =
        classify_directory_with_progress(target_manifest, install_dir, runtime_hints)?;
    let missing_file_count = classification.missing_files.len();
    let corrupt_file_count = classification.corrupt_files.len();
    let extra_file_count = classification.extra_files.len();
    let needs_rebuild = missing_file_count > 0 || corrupt_file_count > 0;

    let mut restored_extra_file_count = 0usize;
    let mut removed_extra_file_count = 0usize;
    let mut preserved_extra_dir = None;

    if needs_rebuild && !remove_added_files && extra_file_count > 0 {
        let preserve_dir = preserve_extra_files(install_dir, target_manifest, &classification)?;
        preserved_extra_dir = Some(preserve_dir);
    }

    let apply_result: Option<TargetInstallApplyResult> = if needs_rebuild {
        let affected_manifest = subset_manifest_for_issues(target_manifest, &classification)?;
        if affected_manifest.files.is_empty() {
            None
        } else {
            if staging_dir.exists() {
                crate::helpers::remove_dir_all(staging_dir)?;
            }
            reconstruct_from_bundle_paths_with_timings_and_hints(
                &affected_manifest,
                bundle_paths,
                staging_dir,
                runtime_hints,
            )?;
            let (staged_errors, _) =
                verify_directory_with_progress(&affected_manifest, staging_dir, runtime_hints)?;
            if !staged_errors.is_empty() {
                return Err(Error::verify(format!(
                    "staged repair files did not verify: {}",
                    staged_errors.join("; ")
                )));
            }
            promote_repaired_files(&affected_manifest, install_dir, staging_dir)?;
            let _ = crate::helpers::remove_dir_all(staging_dir);
            None
        }
    } else {
        None
    };

    if let Some(preserved_extra_dir) = preserved_extra_dir.as_ref() {
        restored_extra_file_count =
            restore_preserved_extra_files(preserved_extra_dir, install_dir, target_manifest)?;
        let _ = crate::helpers::remove_dir_all(preserved_extra_dir);
    }

    if remove_added_files {
        removed_extra_file_count =
            remove_extra_files(install_dir, target_manifest, &classification.extra_files)?;
    }

    let (final_classification, final_verify_timings) =
        classify_directory_with_progress(target_manifest, install_dir, runtime_hints)?;
    if !final_classification.missing_files.is_empty()
        || !final_classification.corrupt_files.is_empty()
        || (remove_added_files && !final_classification.extra_files.is_empty())
    {
        return Err(Error::verify(format!(
            "repaired install did not verify: {}",
            final_classification
                .missing_files
                .into_iter()
                .chain(final_classification.corrupt_files)
                .chain(final_classification.extra_files)
                .map(|issue| issue.detail)
                .collect::<Vec<_>>()
                .join("; ")
        )));
    }

    Ok(TargetInstallRepairResult {
        target_id: target_manifest.target_id.clone(),
        install_dir: install_dir.to_path_buf(),
        missing_file_count,
        corrupt_file_count,
        extra_file_count,
        removed_extra_file_count,
        restored_extra_file_count,
        apply_result,
        verify_timings: if needs_rebuild || remove_added_files {
            final_verify_timings
        } else {
            initial_verify_timings
        },
    })
}

pub fn fetch_target_install_content(
    request: &TargetInstallFetchRequest,
) -> Result<TargetInstallFetchResult> {
    let repo_paths = crate::GutchRepoPaths::from_repo_root(&request.repo_path);
    crate::helpers::create_dir_all(&repo_paths.targets_root)?;
    crate::helpers::create_dir_all(&repo_paths.bundles_root)?;

    let manifest_bytes = download_url(
        request.manifest_url.trim(),
        non_empty_trimmed(request.session_id.as_deref()),
    )?;
    let manifest =
        download_json_bytes::<serde_json::Value>(&manifest_bytes, "downloaded target manifest")?;
    write_repo_manifest(&request.repo_path, &manifest)?;

    if let Some(chunk_index_url) = non_empty_trimmed(request.chunk_index_url.as_deref()) {
        let chunk_index_bytes = download_url(
            chunk_index_url,
            non_empty_trimmed(request.session_id.as_deref()),
        )?;
        let chunk_index = download_json_bytes::<manifest::ChunkIndex>(
            &chunk_index_bytes,
            "downloaded chunk index",
        )?;
        crate::write_chunk_index(&repo_paths, &chunk_index)?;
    }

    let bundle_ids_from_chunk_index: Vec<String> = {
        let chunk_index = crate::load_chunk_index(&repo_paths).ok();

        if let Some(index) = chunk_index {
            let target = target_manifest_from_download(&manifest_bytes, &request.target_id)?;
            crate::repo::required_bundle_ids(&target, &index)
                .into_iter()
                .collect()
        } else {
            Vec::new()
        }
    };
    let target_manifest = target_manifest_from_download(&manifest_bytes, &request.target_id)?;
    crate::write_target_manifest_artifacts(&repo_paths, &target_manifest)?;
    let explicit_bundles = normalize_bundle_downloads(&request.bundles);
    let bundles = if explicit_bundles.is_empty() && !bundle_ids_from_chunk_index.is_empty() {
        fetch_bundle_urls(request, &bundle_ids_from_chunk_index)?
    } else {
        explicit_bundles
    };

    if bundles.is_empty() && bundle_ids_from_chunk_index.is_empty() {
        let required_chunk_count = crate::repo::required_chunk_hashes(&target_manifest).len();
        if required_chunk_count > 0 {
            return Err(Error::bundle(
                "chunk index is required to resolve Gutch bundles for this install",
            ));
        }
    }

    let returned_bundle_ids = bundles
        .iter()
        .map(|bundle| bundle.bundle_id.trim())
        .filter(|bundle_id| !bundle_id.is_empty())
        .map(str::to_string)
        .collect::<HashSet<_>>();
    let missing_bundle_ids = bundle_ids_from_chunk_index
        .iter()
        .filter(|bundle_id| !returned_bundle_ids.contains(bundle_id.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    if !missing_bundle_ids.is_empty() {
        return Err(Error::bundle(format!(
            "bundle URL response is missing {} required bundle(s): {}",
            missing_bundle_ids.len(),
            missing_bundle_ids.join(", ")
        )));
    }

    let total_bundles = bundles.len();
    let mut downloaded_bundle_count = 0usize;
    let mut skipped_existing_bundle_count = 0usize;
    let mut transferred_bytes: u64 = 0;
    for (index, bundle) in bundles.iter().enumerate() {
        let bundle_id = bundle.bundle_id.trim();
        if bundle_id.is_empty() {
            continue;
        }

        let bundle_path = crate::bundle_path(&repo_paths, bundle_id);
        if bundle_path.exists() {
            skipped_existing_bundle_count += 1;
            crate::progress::emit_progress(
                &request.progress,
                crate::GutchProgressEvent {
                    operation: crate::GutchOperation::Fetch,
                    phase: String::from("downloading"),
                    message: format!("Bundle {} of {} (cached)", index + 1, total_bundles),
                    current: index + 1,
                    total: total_bundles,
                    transferred_bytes,
                    total_bytes: 0,
                    detail: Some(bundle_id.to_string()),
                    done: false,
                },
            );
            continue;
        }

        crate::progress::emit_progress(
            &request.progress,
            crate::GutchProgressEvent {
                operation: crate::GutchOperation::Fetch,
                phase: String::from("downloading"),
                message: format!("Downloading bundle {} of {}", index + 1, total_bundles),
                current: index + 1,
                total: total_bundles,
                transferred_bytes,
                total_bytes: 0,
                detail: Some(bundle_id.to_string()),
                done: false,
            },
        );

        let bundle_bytes = download_url(
            bundle.url.trim(),
            non_empty_trimmed(request.session_id.as_deref()),
        )?;
        transferred_bytes += bundle_bytes.len() as u64;
        crate::helpers::write(&bundle_path, bundle_bytes)?;
        downloaded_bundle_count += 1;
    }

    if bundle_ids_from_chunk_index.len() > downloaded_bundle_count + skipped_existing_bundle_count {
        let msg = format!(
            "downloaded {} of {} required bundle(s)",
            downloaded_bundle_count + skipped_existing_bundle_count,
            bundle_ids_from_chunk_index.len()
        );
        crate::progress::emit_progress(
            &request.progress,
            crate::GutchProgressEvent {
                operation: crate::GutchOperation::Fetch,
                phase: String::from("failed"),
                message: msg.clone(),
                current: total_bundles,
                total: total_bundles,
                transferred_bytes,
                total_bytes: 0,
                detail: None,
                done: true,
            },
        );
        return Err(Error::bundle(msg));
    }

    crate::progress::emit_progress(
        &request.progress,
        crate::GutchProgressEvent {
            operation: crate::GutchOperation::Fetch,
            phase: String::from("done"),
            message: format!(
                "Downloaded {} bundle(s), {} cached",
                downloaded_bundle_count, skipped_existing_bundle_count
            ),
            current: total_bundles,
            total: total_bundles,
            transferred_bytes,
            total_bytes: transferred_bytes,
            detail: None,
            done: true,
        },
    );

    Ok(TargetInstallFetchResult {
        target_id: request.target_id.clone(),
        repo_path: request.repo_path.clone(),
        required_bundle_count: bundle_ids_from_chunk_index.len(),
        downloaded_bundle_count,
        skipped_existing_bundle_count,
    })
}

fn normalize_bundle_downloads(bundles: &[GutchBundleDownload]) -> Vec<GutchBundleDownload> {
    bundles
        .iter()
        .filter_map(|bundle| {
            let bundle_id = bundle.bundle_id.trim();
            let url = bundle.url.trim();
            if bundle_id.is_empty() || url.is_empty() {
                None
            } else {
                Some(GutchBundleDownload {
                    bundle_id: bundle_id.to_string(),
                    url: url.to_string(),
                })
            }
        })
        .collect()
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
        crate::helpers::create_dir_all(parent)?;
    }

    let backup_dir = if install_dir.exists() {
        let backup_dir = backup_dir_for_install(install_dir, &target_manifest.target_id);
        if backup_dir.exists() {
            crate::helpers::remove_dir_all(&backup_dir)?;
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
        crate::helpers::remove_dir_all(backup_dir)?;
    }

    Ok(None)
}

fn preserve_extra_files(
    install_dir: &Path,
    target_manifest: &TargetManifest,
    classification: &RepairClassification,
) -> Result<PathBuf> {
    let preserve_dir = install_dir.with_file_name(format!(
        ".gutch-preserve-extra-{}-{}",
        install_dir
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("install"),
        std::process::id()
    ));
    if preserve_dir.exists() {
        crate::helpers::remove_dir_all(&preserve_dir)?;
    }
    crate::helpers::create_dir_all(&preserve_dir)?;

    let manifest_paths = target_manifest
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect::<HashSet<_>>();

    for issue in &classification.extra_files {
        if manifest_paths.contains(issue.path.as_str()) {
            continue;
        }
        let source = crate::path_guard::resolve_manifest_relative_path(install_dir, &issue.path)?;
        if !source.is_file() {
            continue;
        }
        let dest = crate::path_guard::resolve_manifest_relative_path(&preserve_dir, &issue.path)?;
        if let Some(parent) = dest.parent() {
            crate::helpers::create_dir_all(parent)?;
        }
        fs::copy(&source, &dest)?;
    }

    Ok(preserve_dir)
}

fn subset_manifest_for_issues(
    target_manifest: &TargetManifest,
    classification: &RepairClassification,
) -> Result<TargetManifest> {
    let affected_paths = classification
        .missing_files
        .iter()
        .chain(classification.corrupt_files.iter())
        .map(|issue| issue.path.as_str())
        .collect::<HashSet<_>>();
    let mut subset = target_manifest.clone();
    subset.files = target_manifest
        .files
        .iter()
        .filter(|file| affected_paths.contains(file.path.as_str()))
        .cloned()
        .collect();
    subset.total_logical_size = subset.files.iter().map(|file| file.size).sum();
    subset.total_compressed_size_estimate = subset.total_logical_size / 2;
    if !affected_paths.is_empty() && subset.files.is_empty() {
        return Err(Error::verify(
            "repair classification referenced files that are not present in the target manifest",
        ));
    }
    Ok(subset)
}

fn promote_repaired_files(
    repaired_manifest: &TargetManifest,
    install_dir: &Path,
    staging_dir: &Path,
) -> Result<()> {
    for file in &repaired_manifest.files {
        let source = crate::path_guard::resolve_manifest_relative_path(staging_dir, &file.path)?;
        let dest = crate::path_guard::resolve_manifest_relative_path(install_dir, &file.path)?;
        if let Some(parent) = dest.parent() {
            crate::helpers::create_dir_all(parent)?;
        }
        if dest.exists() {
            crate::helpers::remove_file(&dest)?;
        }
        fs::rename(&source, &dest)?;
    }
    Ok(())
}

fn restore_preserved_extra_files(
    preserve_dir: &Path,
    install_dir: &Path,
    target_manifest: &TargetManifest,
) -> Result<usize> {
    let manifest_paths = target_manifest
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect::<HashSet<_>>();
    let mut restored = 0usize;

    for entry in walkdir::WalkDir::new(preserve_dir).sort_by_file_name() {
        let entry = entry?;
        if !entry.path().is_file() {
            continue;
        }
        let Ok(relative_path) = entry.path().strip_prefix(preserve_dir) else {
            continue;
        };
        let relative = relative_path.to_string_lossy().replace('\\', "/");
        if manifest_paths.contains(relative.as_str()) {
            continue;
        }
        let dest = crate::path_guard::resolve_manifest_relative_path(install_dir, &relative)?;
        if dest.exists() {
            continue;
        }
        if let Some(parent) = dest.parent() {
            crate::helpers::create_dir_all(parent)?;
        }
        fs::copy(entry.path(), &dest)?;
        restored += 1;
    }

    Ok(restored)
}

fn remove_extra_files(
    install_dir: &Path,
    target_manifest: &TargetManifest,
    extra_files: &[RepairIssue],
) -> Result<usize> {
    let manifest_paths = target_manifest
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect::<HashSet<_>>();
    let mut removed = 0usize;

    for issue in extra_files {
        if manifest_paths.contains(issue.path.as_str()) {
            continue;
        }
        let path = crate::path_guard::resolve_manifest_relative_path(install_dir, &issue.path)?;
        if path.is_file() {
            crate::helpers::remove_file(&path)?;
            removed += 1;
            remove_empty_parent_dirs(install_dir, path.parent());
        }
    }

    Ok(removed)
}

fn remove_empty_parent_dirs(root: &Path, mut current: Option<&Path>) {
    while let Some(path) = current {
        if path == root {
            break;
        }
        match fs::remove_dir(path) {
            Ok(()) => current = path.parent(),
            Err(_) => break,
        }
    }
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

fn download_url(url: &str, session_id: Option<&str>) -> Result<Vec<u8>> {
    if url.is_empty() {
        return Err(Error::bundle("download URL is required"));
    }

    let client = reqwest::blocking::Client::new();
    let mut builder = client.get(url);
    if let Some(session_id) = session_id.filter(|_| should_attach_bearer_session(url)) {
        builder = builder.header("Authorization", format!("Bearer {}", session_id));
    }

    let response = builder
        .send()
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

fn should_attach_bearer_session(url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return false;
    };
    if !parsed.path().starts_with("/v1/") {
        return false;
    }
    matches!(
        parsed.host_str().unwrap_or_default(),
        "localhost" | "127.0.0.1" | "api.example.invalid"
    )
}

fn download_json_bytes<T: serde::de::DeserializeOwned>(
    bytes: &[u8],
    description: &str,
) -> Result<T> {
    serde_json::from_slice(bytes)
        .map_err(|err| Error::manifest(format!("{description} is invalid JSON: {err}")))
}

fn target_manifest_from_download(bytes: &[u8], fallback_target_id: &str) -> Result<TargetManifest> {
    let mut manifest = download_json_bytes::<TargetManifest>(bytes, "downloaded target manifest")?;
    if manifest.target_id.trim().is_empty() {
        manifest.target_id = fallback_target_id.to_string();
    }
    Ok(manifest)
}

fn non_empty_trimmed(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn fetch_bundle_urls(
    request: &TargetInstallFetchRequest,
    bundle_ids: &[String],
) -> Result<Vec<GutchBundleDownload>> {
    let endpoint = request
        .bundle_downloads_endpoint
        .as_deref()
        .and_then(|value| non_empty_trimmed(Some(value)))
        .ok_or_else(|| Error::bundle("bundle URL endpoint is required for Gutch install"))?;
    let separator = if endpoint.contains('?') { '&' } else { '?' };
    let url = format!("{}{}ids={}", endpoint, separator, bundle_ids.join(","));
    let mut builder = reqwest::blocking::Client::new().get(url);
    if let Some(session_id) = non_empty_trimmed(request.session_id.as_deref()) {
        builder = builder.header("Authorization", format!("Bearer {}", session_id));
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
    let repo_name = if existing_manifest_path.exists() {
        let json = std::fs::read_to_string(&existing_manifest_path)?;
        let existing_manifest: crate::manifest::gutch_repo::GutchRepoManifest =
            serde_json::from_str(&json)?;
        if existing_manifest.name.is_empty() {
            repo_path
                .file_name()
                .and_then(|value| value.to_str())
                .filter(|value| !value.is_empty())
                .unwrap_or("base")
                .to_string()
        } else {
            existing_manifest.name
        }
    } else {
        repo_path
            .file_name()
            .and_then(|value| value.to_str())
            .filter(|value| !value.is_empty())
            .unwrap_or("base")
            .to_string()
    };
    let mut repo_manifest = crate::manifest::gutch_repo::GutchRepoManifest::new(repo_name);
    if let Some(inserted_at) = target_manifest
        .get("inserted_at")
        .and_then(serde_json::Value::as_str)
    {
        repo_manifest.created_at = inserted_at.to_string();
    }

    let repo_json = serde_json::to_vec_pretty(&repo_manifest)?;
    crate::helpers::write(
        &repo_path.join(manifest::CONTENT_REPO_MANIFEST_FILE),
        repo_json,
    )?;
    Ok(())
}

#[cfg(test)]
#[path = "install_tests.rs"]
mod tests;

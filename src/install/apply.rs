use std::path::{Path, PathBuf};

use crate::manifest::target::TargetManifest;
use crate::reconstruct::VerifiedSourceInstall;
use crate::runtime::RuntimeResourceHints;

use super::types::{
    TargetInstallApplyFromRepoRequest, TargetInstallApplyFromRepoResult, TargetInstallApplyResult,
    TargetInstallApplySource, TargetInstallVerifyFromRepoRequest,
    TargetInstallVerifyFromRepoResult,
};
use crate::verify::{VerifyRequest, classify_directory};
use crate::{Error, Result};

pub(super) fn resolve_verified_source_install<'a>(
    source: Option<TargetInstallApplySource<'a>>,
    runtime_hints: &RuntimeResourceHints,
    control: &crate::GutchOperationControl,
) -> Result<Option<VerifiedSourceInstall<'a>>> {
    control.checkpoint()?;
    match source {
        Some(source) => {
            let (source_classification, _) = classify_directory(VerifyRequest {
                target_manifest: source.target_manifest,
                dir: source.install_dir,
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
                return Err(Error::verify(format!(
                    "source install did not verify: {}",
                    source_errors
                )));
            }

            Ok(Some(VerifiedSourceInstall {
                target_manifest: source.target_manifest,
                install_dir: source.install_dir,
            }))
        }
        None => Ok(None),
    }
}

pub fn default_target_install_staging_dir(install_dir: &Path, target_id: &str) -> PathBuf {
    let install_name = install_dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("install");
    install_dir.with_file_name(format!(".gutch-staging-{}-{}", install_name, target_id))
}

pub fn verify_target_install_from_repo(
    request: TargetInstallVerifyFromRepoRequest,
) -> Result<TargetInstallVerifyFromRepoResult> {
    request.control.checkpoint()?;
    let (repo_paths, target_manifest) =
        super::repo_target::load_repo_target_manifest(&request.repo_path, &request.target_id)?;
    let (classification, verify_timings) = classify_directory(VerifyRequest {
        target_manifest: &target_manifest,
        dir: &request.install_dir,
        mode: request.verify_mode,
        runtime_hints: &request.runtime_hints,
        control: request.control,
    })?;

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
    request.control.checkpoint()?;
    let (repo_paths, target_manifest) =
        super::repo_target::load_repo_target_manifest(&request.repo_path, &request.target_id)?;
    let chunk_index = crate::load_chunk_index(&repo_paths)?;
    let existing_bundle_ids = crate::repo::required_bundle_ids(&target_manifest, &chunk_index)
        .into_iter()
        .filter(|bundle_id| crate::bundle_path(&repo_paths, bundle_id).is_file())
        .collect();

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
    let verified_source =
        resolve_verified_source_install(source, &request.runtime_hints, &request.control)?;
    let pipeline_plan = super::pipeline::build_install_pipeline_plan(
        &target_manifest,
        &chunk_index,
        &repo_paths,
        &existing_bundle_ids,
        verified_source,
    )?;
    super::pipeline::apply_install_pipeline(
        super::pipeline::InstallPipelineApplyRequest {
            repo_root: &repo_paths.repo_root,
            target_manifest: &target_manifest,
            install_dir: &request.install_dir,
            staging_dir: request.staging_dir.as_deref(),
            source_install_dir,
            runtime_hints: &request.runtime_hints,
            existing_bundle_ids: &existing_bundle_ids,
            pending_bundle_downloads: &[],
            control: &request.control,
            download_client: None,
            transfer_progress: None,
            session_id: None,
        },
        pipeline_plan,
    )
}

pub(super) fn build_target_install_apply_result(
    target_manifest: &TargetManifest,
    install_dir: &Path,
    staging_dir: &Path,
    backup_dir: Option<PathBuf>,
    reused_source_chunk_count: usize,
    required_chunk_count: usize,
    required_bundle_count: usize,
    reconstruct_timings: crate::ReconstructTimings,
    verify_timings: crate::verify::VerifyTimings,
) -> TargetInstallApplyResult {
    TargetInstallApplyResult {
        target_id: target_manifest.target_id.clone(),
        install_dir: install_dir.to_path_buf(),
        staging_dir: staging_dir.to_path_buf(),
        backup_dir,
        reused_source_chunk_count,
        required_chunk_count,
        required_bundle_count,
        reconstruct_timings,
        verify_timings,
    }
}

pub(super) fn promote_staging_install(
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

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

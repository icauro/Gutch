use std::path::{Path, PathBuf};

use crate::manifest::TargetManifest;
use crate::reconstruct::{
    ReconstructTimings, VerifiedSourceInstall,
    reconstruct_from_bundle_paths_and_verified_source_with_timings,
};
use crate::verify::{
    VerifyTimings, classify_directory_with_timings, verify_directory_with_timings,
};
use crate::{Error, Result};

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

pub fn apply_target_install_with_timings(
    target_manifest: &TargetManifest,
    bundle_paths: &[PathBuf],
    install_dir: &Path,
    staging_dir: &Path,
    source: Option<TargetInstallApplySource<'_>>,
) -> Result<TargetInstallApplyResult> {
    if install_dir == staging_dir {
        return Err(Error::PathError {
            path: install_dir.to_path_buf(),
            reason: "install and staging directories must be different".to_string(),
        });
    }

    let verified_source = match source {
        Some(source) => {
            let (source_classification, _) =
                classify_directory_with_timings(source.target_manifest, source.install_dir)?;
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

    let (_, reconstruct_timings) = reconstruct_from_bundle_paths_and_verified_source_with_timings(
        target_manifest,
        bundle_paths,
        verified_source,
        staging_dir,
    )?;

    let (verify_errors, verify_timings) =
        verify_directory_with_timings(target_manifest, staging_dir)?;
    if !verify_errors.is_empty() {
        return Err(Error::verify(format!(
            "staged install did not verify: {}",
            verify_errors.join("; ")
        )));
    }

    let backup_dir = promote_staging_install(install_dir, staging_dir, target_manifest)?;

    Ok(TargetInstallApplyResult {
        target_id: target_manifest.target_id.clone(),
        install_dir: install_dir.to_path_buf(),
        staging_dir: staging_dir.to_path_buf(),
        backup_dir,
        reused_source_chunk_count,
        required_chunk_count: target_manifest.required_chunk_set.len(),
        required_bundle_count: target_manifest.required_bundle_set.len(),
        reconstruct_timings,
        verify_timings,
    })
}

fn count_reusable_source_chunks(
    target_manifest: &TargetManifest,
    source_manifest: &TargetManifest,
) -> usize {
    let target_chunks: std::collections::HashSet<&str> = target_manifest
        .required_chunk_set
        .iter()
        .map(String::as_str)
        .collect();
    source_manifest
        .required_chunk_set
        .iter()
        .filter(|chunk_hash| target_chunks.contains(chunk_hash.as_str()))
        .count()
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

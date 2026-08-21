use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::manifest::target::TargetManifest;
use crate::runtime::RuntimeResourceHints;
use crate::verify::{RepairClassification, RepairIssue, VerifyRequest, classify_directory};
use crate::{Error, Result};

use super::apply::default_target_install_staging_dir;
use super::pipeline::{build_install_pipeline_plan, execute_install_pipeline_materialization};
use super::types::{
    TargetInstallRepairFromRepoRequest, TargetInstallRepairFromRepoResult,
    TargetInstallRepairPlanFromRepoRequest, TargetInstallRepairPlanFromRepoResult,
    TargetInstallRepairResult,
};

struct PlannedRepair {
    classification: RepairClassification,
    affected_manifest: TargetManifest,
    missing_file_count: usize,
    corrupt_file_count: usize,
    extra_file_count: usize,
    needs_rebuild: bool,
    required_bundle_ids: HashSet<String>,
}

fn manifest_paths_set(target_manifest: &TargetManifest) -> HashSet<&str> {
    target_manifest
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect()
}

pub fn repair_target_install_from_repo(
    request: TargetInstallRepairFromRepoRequest,
) -> Result<TargetInstallRepairFromRepoResult> {
    request.control.checkpoint()?;
    let (repo_paths, target_manifest) =
        super::repo_target::load_repo_target_manifest(&request.repo_path, &request.target_id)?;
    let (_, repo_manifest) = crate::load_gutch_repo_manifest(&repo_paths.repo_root)?;
    let chunk_index = crate::load_chunk_index(&repo_paths)?;
    let repair_plan = plan_repair_target_install(
        &target_manifest,
        Some(&chunk_index),
        &request.install_dir,
        request.remove_added_files,
        &request.runtime_hints,
        &request.control,
    )?;
    let staging_dir = request.staging_dir.unwrap_or_else(|| {
        default_target_install_staging_dir(&request.install_dir, &target_manifest.target_id)
    });
    let resolved_bundle_file_count = repair_plan.required_bundle_ids.len();
    if repair_plan.needs_rebuild {
        let existing_bundle_ids = repair_plan
            .required_bundle_ids
            .iter()
            .filter(|bundle_id| crate::bundle_path(&repo_paths, bundle_id).is_file())
            .cloned()
            .collect::<HashSet<_>>();
        let pipeline_plan = build_install_pipeline_plan(
            &repair_plan.affected_manifest,
            &chunk_index,
            &repo_paths,
            &existing_bundle_ids,
            None,
        )?;
        let _ = execute_install_pipeline_materialization(
            pipeline_plan,
            &staging_dir,
            &target_manifest.target_id,
            &repo_manifest.compression_profile(),
            &request.runtime_hints,
            &existing_bundle_ids,
            &[],
            &request.control,
            None,
            None,
            None,
        )?;
    }
    let repair_result = finalize_repair_result_after_materialization(
        &target_manifest,
        &request.install_dir,
        &staging_dir,
        request.remove_added_files,
        &request.runtime_hints,
        &repair_plan,
        &request.control,
    )?;

    Ok(TargetInstallRepairFromRepoResult {
        repo_root: repo_paths.repo_root,
        target_manifest,
        repair_result,
        resolved_bundle_file_count,
    })
}

pub fn plan_repair_target_install_from_repo(
    request: TargetInstallRepairPlanFromRepoRequest,
) -> Result<TargetInstallRepairPlanFromRepoResult> {
    request.control.checkpoint()?;
    let (repo_paths, target_manifest) =
        super::repo_target::load_repo_target_manifest(&request.repo_path, &request.target_id)?;
    let chunk_index = crate::load_chunk_index(&repo_paths)?;
    let repair_plan = plan_repair_target_install(
        &target_manifest,
        Some(&chunk_index),
        &request.install_dir,
        request.remove_added_files,
        &request.runtime_hints,
        &request.control,
    )?;

    Ok(TargetInstallRepairPlanFromRepoResult {
        repo_root: repo_paths.repo_root,
        target_manifest,
        missing_file_count: repair_plan.missing_file_count,
        corrupt_file_count: repair_plan.corrupt_file_count,
        extra_file_count: repair_plan.extra_file_count,
        needs_rebuild: repair_plan.needs_rebuild,
        required_bundle_ids: repair_plan.required_bundle_ids.into_iter().collect(),
    })
}

fn plan_repair_target_install(
    target_manifest: &TargetManifest,
    chunk_index: Option<&crate::manifest::ChunkIndex>,
    install_dir: &Path,
    _remove_added_files: bool,
    runtime_hints: &RuntimeResourceHints,
    control: &crate::GutchOperationControl,
) -> Result<PlannedRepair> {
    let (classification, _) = classify_directory(VerifyRequest {
        target_manifest,
        dir: install_dir,
        mode: crate::VerifyMode::Fast,
        runtime_hints,
        control: control.clone(),
    })?;
    build_repair_plan_from_classification(target_manifest, chunk_index, classification)
}

fn build_repair_plan_from_classification(
    target_manifest: &TargetManifest,
    chunk_index: Option<&crate::manifest::ChunkIndex>,
    classification: RepairClassification,
) -> Result<PlannedRepair> {
    let missing_file_count = classification.missing_files.len();
    let corrupt_file_count = classification.corrupt_files.len();
    let extra_file_count = classification.extra_files.len();
    let needs_rebuild = missing_file_count > 0 || corrupt_file_count > 0;
    let affected_manifest = subset_manifest_for_issues(target_manifest, &classification)?;
    let required_bundle_ids = if needs_rebuild {
        chunk_index
            .map(|index| crate::repo::required_bundle_ids(&affected_manifest, index))
            .unwrap_or_default()
    } else {
        HashSet::new()
    };

    Ok(PlannedRepair {
        classification,
        affected_manifest,
        missing_file_count,
        corrupt_file_count,
        extra_file_count,
        needs_rebuild,
        required_bundle_ids,
    })
}

fn finalize_repair_result_after_materialization(
    target_manifest: &TargetManifest,
    install_dir: &Path,
    staging_dir: &Path,
    remove_added_files: bool,
    runtime_hints: &RuntimeResourceHints,
    repair_plan: &PlannedRepair,
    control: &crate::GutchOperationControl,
) -> Result<TargetInstallRepairResult> {
    let mut restored_extra_file_count = 0usize;
    let mut removed_extra_file_count = 0usize;
    let mut preserved_extra_dir = None;

    // Everything below this checkpoint is one consistency-critical local commit. Cancellation
    // arriving during promotion is observed by the caller only after a valid install is active.
    control.checkpoint()?;
    if !remove_added_files && repair_plan.extra_file_count > 0 {
        preserved_extra_dir = Some(preserve_extra_files(
            install_dir,
            target_manifest,
            &repair_plan.classification,
        )?);
    }

    promote_repaired_files(&repair_plan.affected_manifest, install_dir, staging_dir)?;
    let _ = crate::helpers::remove_dir_all(staging_dir);

    if let Some(preserved_extra_dir) = preserved_extra_dir.as_ref() {
        restored_extra_file_count =
            restore_preserved_extra_files(preserved_extra_dir, install_dir, target_manifest)?;
        let _ = crate::helpers::remove_dir_all(preserved_extra_dir);
    }

    if remove_added_files {
        removed_extra_file_count = remove_extra_files(
            install_dir,
            target_manifest,
            &repair_plan.classification.extra_files,
        )?;
    }

    let (final_classification, final_verify_timings) = classify_directory(VerifyRequest {
        target_manifest,
        dir: install_dir,
        mode: crate::VerifyMode::Fast,
        runtime_hints,
        control: crate::GutchOperationControl::default(),
    })?;
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
        missing_file_count: repair_plan.missing_file_count,
        corrupt_file_count: repair_plan.corrupt_file_count,
        extra_file_count: repair_plan.extra_file_count,
        removed_extra_file_count,
        restored_extra_file_count,
        apply_result: None,
        verify_timings: final_verify_timings,
    })
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

    let manifest_paths = manifest_paths_set(target_manifest);

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
    let manifest_paths = manifest_paths_set(target_manifest);
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
    let manifest_paths = manifest_paths_set(target_manifest);
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

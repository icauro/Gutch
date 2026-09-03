use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::manifest::{SnapshotEntry, snapshot::SnapshotManifest};
use crate::runtime::{RuntimeResourceHints, plan_reconstruct_runtime};
use crate::verify::{RepairClassification, RepairIssue, VerifyRequest, classify_directory};
use crate::{Error, Result};

use super::apply::default_snapshot_staging_dir;
use super::pipeline::{
    ExecuteReconstructPipelineRequest, build_reconstruct_pipeline_plan, build_reconstruct_pool,
    execute_reconstruct_pipeline_reconstruction, prepare_snapshot_entries,
};
use super::types::{
    SnapshotRepairFromRepoRequest, SnapshotRepairFromRepoResult, SnapshotRepairPlanFromRepoRequest,
    SnapshotRepairPlanFromRepoResult, SnapshotRepairResult,
};

struct PlannedRepair {
    classification: RepairClassification,
    affected_manifest: SnapshotManifest,
    missing_file_count: usize,
    corrupt_file_count: usize,
    extra_file_count: usize,
    needs_rebuild: bool,
    required_artifacts: HashSet<crate::ArtifactRef>,
}

fn manifest_paths_set(snapshot_manifest: &SnapshotManifest) -> HashSet<&str> {
    snapshot_manifest
        .entries
        .iter()
        .map(|entry| entry.path())
        .collect()
}

pub fn repair_snapshot_from_repo(
    request: SnapshotRepairFromRepoRequest,
) -> Result<SnapshotRepairFromRepoResult> {
    request.control.checkpoint()?;
    let (repo_paths, snapshot_manifest) = super::snapshot_repo::load_repo_snapshot_manifest(
        &request.repo_path,
        &request.snapshot_id,
    )?;
    let (_, repo_manifest) = crate::load_gutch_repo_manifest(&repo_paths.repo_root)?;
    let chunk_index = crate::load_chunk_index(&repo_paths)?;
    let repair_plan = plan_repair_snapshot_ops(
        &snapshot_manifest,
        Some(&chunk_index),
        &request.destination_dir,
        request.remove_added_files,
        &request.runtime_hints,
        &request.control,
    )?;
    let staging_dir = request.staging_dir.unwrap_or_else(|| {
        default_snapshot_staging_dir(&request.destination_dir, &snapshot_manifest.snapshot_id)
    });
    let resolved_bundle_file_count = repair_plan
        .required_artifacts
        .iter()
        .filter(|artifact| matches!(artifact, crate::ArtifactRef::Bundle { .. }))
        .count();
    if repair_plan.needs_rebuild {
        let mut existing_artifacts = HashSet::new();
        for artifact in &repair_plan.required_artifacts {
            if artifact.local_path(&repo_paths)?.is_file() {
                existing_artifacts.insert(artifact.clone());
            }
        }
        let pipeline_plan = {
            let _plan_span = request
                .control
                .profile_span("gutch.snapshot.reconstruct.prepare.plan");
            build_reconstruct_pipeline_plan(
                &repair_plan.affected_manifest,
                &chunk_index,
                &repo_paths,
                &existing_artifacts,
                None,
            )?
        };
        let compression_profile = repo_manifest.compression_profile();
        let reconstruct_pool = if repair_plan.affected_manifest.entries.is_empty() {
            None
        } else {
            Some(build_reconstruct_pool(
                plan_reconstruct_runtime(&request.runtime_hints).worker_count,
            )?)
        };
        if staging_dir.exists() {
            crate::helpers::remove_dir_all(&staging_dir)?;
        }
        if let Some(parent) = staging_dir.parent() {
            crate::helpers::create_dir_all(parent)?;
        }
        prepare_snapshot_entries(
            &repair_plan.affected_manifest,
            &staging_dir,
            &request.control,
            reconstruct_pool.as_ref(),
        )?;
        let _ = execute_reconstruct_pipeline_reconstruction(ExecuteReconstructPipelineRequest {
            pipeline_plan,
            staging_dir: &staging_dir,
            snapshot_id: &snapshot_manifest.snapshot_id,
            compression_profile: &compression_profile,
            runtime_hints: &request.runtime_hints,
            existing_artifacts: &existing_artifacts,
            pending_bundle_downloads: &[],
            transfer_policy: super::types::TransferConcurrencyPolicy::default(),
            control: &request.control,
            download_client: None,
            transfer_progress: None,
            reconstruct_pool: reconstruct_pool.as_ref(),
        })?;
    }
    let repair_result = finalize_repair_result_after_reconstruction(
        &snapshot_manifest,
        &request.destination_dir,
        &staging_dir,
        request.remove_added_files,
        &request.runtime_hints,
        &repair_plan,
        &request.control,
    )?;

    Ok(SnapshotRepairFromRepoResult {
        repo_root: repo_paths.repo_root,
        snapshot_manifest,
        repair_result,
        resolved_bundle_file_count,
    })
}

pub fn plan_repair_snapshot_from_repo(
    request: SnapshotRepairPlanFromRepoRequest,
) -> Result<SnapshotRepairPlanFromRepoResult> {
    request.control.checkpoint()?;
    let (repo_paths, snapshot_manifest) = super::snapshot_repo::load_repo_snapshot_manifest(
        &request.repo_path,
        &request.snapshot_id,
    )?;
    let chunk_index = crate::load_chunk_index(&repo_paths)?;
    let repair_plan = plan_repair_snapshot_ops(
        &snapshot_manifest,
        Some(&chunk_index),
        &request.destination_dir,
        request.remove_added_files,
        &request.runtime_hints,
        &request.control,
    )?;

    Ok(SnapshotRepairPlanFromRepoResult {
        repo_root: repo_paths.repo_root,
        snapshot_manifest,
        missing_file_count: repair_plan.missing_file_count,
        corrupt_file_count: repair_plan.corrupt_file_count,
        extra_file_count: repair_plan.extra_file_count,
        needs_rebuild: repair_plan.needs_rebuild,
        required_bundle_ids: repair_plan
            .required_artifacts
            .into_iter()
            .map(|artifact| artifact.id().to_string())
            .collect(),
    })
}

fn plan_repair_snapshot_ops(
    snapshot_manifest: &SnapshotManifest,
    chunk_index: Option<&crate::manifest::ChunkIndex>,
    destination_dir: &Path,
    _remove_added_files: bool,
    runtime_hints: &RuntimeResourceHints,
    control: &crate::GutchOperationControl,
) -> Result<PlannedRepair> {
    let (classification, _) = classify_directory(VerifyRequest {
        snapshot_manifest,
        dir: destination_dir,
        mode: crate::VerifyMode::Fast,
        runtime_hints,
        control: control.clone(),
    })?;
    build_repair_plan_from_classification(snapshot_manifest, chunk_index, classification)
}

fn build_repair_plan_from_classification(
    snapshot_manifest: &SnapshotManifest,
    chunk_index: Option<&crate::manifest::ChunkIndex>,
    classification: RepairClassification,
) -> Result<PlannedRepair> {
    let missing_file_count = classification.missing_files.len();
    let corrupt_file_count = classification.corrupt_files.len();
    let extra_file_count = classification.extra_files.len();
    let needs_rebuild = missing_file_count > 0 || corrupt_file_count > 0;
    let affected_manifest = subset_manifest_for_issues(snapshot_manifest, &classification)?;
    let required_artifacts = if needs_rebuild {
        chunk_index
            .map(|index| crate::required_artifacts(&affected_manifest, index))
            .transpose()?
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
        required_artifacts,
    })
}

fn finalize_repair_result_after_reconstruction(
    snapshot_manifest: &SnapshotManifest,
    destination_dir: &Path,
    staging_dir: &Path,
    remove_added_files: bool,
    runtime_hints: &RuntimeResourceHints,
    repair_plan: &PlannedRepair,
    control: &crate::GutchOperationControl,
) -> Result<SnapshotRepairResult> {
    let mut reconstructed_extra_file_count = 0usize;
    let mut removed_extra_file_count = 0usize;
    let mut preserved_extra_dir = None;

    // Everything below this checkpoint is one consistency-critical local commit. Cancellation
    // arriving during promotion is observed by the caller only after a valid destination is active.
    control.checkpoint()?;
    if !remove_added_files && repair_plan.extra_file_count > 0 {
        preserved_extra_dir = Some(preserve_extra_files(
            destination_dir,
            snapshot_manifest,
            &repair_plan.classification,
        )?);
    }

    promote_repaired_files(&repair_plan.affected_manifest, destination_dir, staging_dir)?;
    let _ = crate::helpers::remove_dir_all(staging_dir);

    if let Some(preserved_extra_dir) = preserved_extra_dir.as_ref() {
        reconstructed_extra_file_count = reconstruct_preserved_extra_files(
            preserved_extra_dir,
            destination_dir,
            snapshot_manifest,
        )?;
        let _ = crate::helpers::remove_dir_all(preserved_extra_dir);
    }

    if remove_added_files {
        removed_extra_file_count = remove_extra_files(
            destination_dir,
            snapshot_manifest,
            &repair_plan.classification.extra_files,
        )?;
    }

    let (final_classification, _final_verify_timings) = classify_directory(VerifyRequest {
        snapshot_manifest,
        dir: destination_dir,
        mode: crate::VerifyMode::Fast,
        runtime_hints,
        control: crate::GutchOperationControl::default(),
    })?;
    if !final_classification.missing_files.is_empty()
        || !final_classification.corrupt_files.is_empty()
        || (remove_added_files && !final_classification.extra_files.is_empty())
    {
        return Err(Error::verify(format!(
            "repaired destination did not verify: {}",
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

    Ok(SnapshotRepairResult {
        snapshot_id: snapshot_manifest.snapshot_id.clone(),
        destination_dir: destination_dir.to_path_buf(),
        missing_file_count: repair_plan.missing_file_count,
        corrupt_file_count: repair_plan.corrupt_file_count,
        extra_file_count: repair_plan.extra_file_count,
        removed_extra_file_count,
        reconstructed_extra_file_count,
        reconstruct_result: None,
    })
}

fn preserve_extra_files(
    destination_dir: &Path,
    snapshot_manifest: &SnapshotManifest,
    classification: &RepairClassification,
) -> Result<PathBuf> {
    let preserve_dir = destination_dir.with_file_name(format!(
        ".gutch-preserve-extra-{}-{}",
        destination_dir
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("destination"),
        std::process::id()
    ));
    if preserve_dir.exists() {
        crate::helpers::remove_dir_all(&preserve_dir)?;
    }
    crate::helpers::create_dir_all(&preserve_dir)?;

    let manifest_paths = manifest_paths_set(snapshot_manifest);

    for issue in &classification.extra_files {
        if manifest_paths.contains(issue.path.as_str()) {
            continue;
        }
        let source =
            crate::path_guard::resolve_manifest_relative_path(destination_dir, &issue.path)?;
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
    snapshot_manifest: &SnapshotManifest,
    classification: &RepairClassification,
) -> Result<SnapshotManifest> {
    let affected_paths = classification
        .missing_files
        .iter()
        .chain(classification.corrupt_files.iter())
        .map(|issue| issue.path.as_str())
        .collect::<HashSet<_>>();
    let mut subset = snapshot_manifest.clone();
    let mut required_paths = affected_paths.clone();
    loop {
        let previous_len = required_paths.len();
        for entry in &snapshot_manifest.entries {
            if required_paths.contains(entry.path())
                && let SnapshotEntry::Symlink(link) = entry
            {
                required_paths.insert(link.target_path.as_str());
            }
        }
        if required_paths.len() == previous_len {
            break;
        }
    }
    subset.entries = snapshot_manifest
        .entries
        .iter()
        .filter(|entry| required_paths.contains(entry.path()))
        .cloned()
        .collect();
    subset.total_logical_size = subset.files().map(|file| file.size).sum();
    subset.total_compressed_size_estimate = subset.total_logical_size / 2;
    if !affected_paths.is_empty() && subset.entries.is_empty() {
        return Err(Error::verify(
            "repair classification referenced entries that are not present in the snapshot manifest",
        ));
    }
    Ok(subset)
}

fn promote_repaired_files(
    repaired_manifest: &SnapshotManifest,
    destination_dir: &Path,
    staging_dir: &Path,
) -> Result<()> {
    let mut directories = repaired_manifest
        .entries
        .iter()
        .filter_map(|entry| match entry {
            SnapshotEntry::Directory(directory) => Some(directory),
            _ => None,
        })
        .collect::<Vec<_>>();
    directories.sort_by_key(|directory| directory.path.matches('/').count());
    for directory in &directories {
        let dest =
            crate::path_guard::resolve_manifest_relative_path(destination_dir, &directory.path)?;
        let is_existing_directory = fs::symlink_metadata(&dest)
            .map(|metadata| metadata.file_type().is_dir() && !metadata.file_type().is_symlink())
            .unwrap_or(false);
        if !is_existing_directory {
            remove_existing_entry(&dest)?;
        }
        crate::helpers::create_dir_all(&dest)?;
    }

    for entry in &repaired_manifest.entries {
        let path = match entry {
            SnapshotEntry::File(file) => &file.path,
            SnapshotEntry::Symlink(link) => &link.path,
            SnapshotEntry::Directory(_) => continue,
        };
        let source = crate::path_guard::resolve_manifest_relative_path(staging_dir, path)?;
        let dest = crate::path_guard::resolve_manifest_relative_path(destination_dir, path)?;
        if let Some(parent) = dest.parent() {
            crate::helpers::create_dir_all(parent)?;
        }
        remove_existing_entry(&dest)?;
        fs::rename(&source, &dest)?;
    }

    directories.sort_by_key(|directory| std::cmp::Reverse(directory.path.matches('/').count()));
    for directory in directories {
        let dest =
            crate::path_guard::resolve_manifest_relative_path(destination_dir, &directory.path)?;
        let mut permissions = fs::metadata(&dest)?.permissions();
        permissions.set_readonly(directory.read_only);
        fs::set_permissions(dest, permissions)?;
    }
    Ok(())
}

fn remove_existing_entry(path: &Path) -> Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
        crate::helpers::remove_dir_all(path)
    } else {
        crate::helpers::remove_file(path)
    }
}

fn reconstruct_preserved_extra_files(
    preserve_dir: &Path,
    destination_dir: &Path,
    snapshot_manifest: &SnapshotManifest,
) -> Result<usize> {
    let manifest_paths = manifest_paths_set(snapshot_manifest);
    let mut reconstructed = 0usize;

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
        let dest = crate::path_guard::resolve_manifest_relative_path(destination_dir, &relative)?;
        if dest.exists() {
            continue;
        }
        if let Some(parent) = dest.parent() {
            crate::helpers::create_dir_all(parent)?;
        }
        fs::copy(entry.path(), &dest)?;
        reconstructed += 1;
    }

    Ok(reconstructed)
}

fn remove_extra_files(
    destination_dir: &Path,
    snapshot_manifest: &SnapshotManifest,
    extra_files: &[RepairIssue],
) -> Result<usize> {
    let manifest_paths = manifest_paths_set(snapshot_manifest);
    let mut removed = 0usize;

    let mut paths = extra_files.iter().collect::<Vec<_>>();
    paths.sort_by_key(|issue| std::cmp::Reverse(issue.path.matches('/').count()));
    for issue in paths {
        if manifest_paths.contains(issue.path.as_str()) {
            continue;
        }
        let path = crate::path_guard::resolve_manifest_relative_path(destination_dir, &issue.path)?;
        if fs::symlink_metadata(&path).is_ok() {
            remove_existing_entry(&path)?;
            removed += 1;
            remove_empty_parent_dirs(destination_dir, path.parent());
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

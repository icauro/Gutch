//! Installation publication policies. Selective publication is retryable, not a multi-file atomic commit.
use super::entries::finalize_snapshot_entries;
use crate::Error;
use crate::Result;
use crate::installation::contracts::SnapshotRepairResult;
use crate::installation::inspect::RepairClassification;
use crate::installation::inspect::RepairIssue;
use crate::installation::inspect::RepairIssueKind;
use crate::installation::inspect::VerifyRequest;
use crate::installation::inspect::classify_expected_entries_with_marker_namespace_and_pool;
use crate::installation::plan::InstallationChanges;
use crate::manifest::SnapshotEntry;
use crate::manifest::snapshot::SnapshotManifest;
use crate::runtime::RuntimeResourceHints;
use rayon::ThreadPool;
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

pub(crate) enum Publication<'a> {
    ReplaceDirectory,
    Selective {
        target: &'a SnapshotManifest,
        changes: &'a crate::installation::plan::InstallationChanges,
        remove_extra_files: bool,
    },
}

fn manifest_paths_set(snapshot_manifest: &SnapshotManifest) -> HashSet<&str> {
    snapshot_manifest
        .entries
        .iter()
        .map(|entry| entry.path())
        .collect()
}

pub(crate) struct FinalizeRepairRequest<'a> {
    pub(crate) snapshot_manifest: &'a SnapshotManifest,
    pub(crate) destination_dir: &'a Path,
    pub(crate) staging_dir: &'a Path,
    pub(crate) remove_added_files: bool,
    pub(crate) runtime_hints: &'a RuntimeResourceHints,
    pub(crate) repair_plan: &'a InstallationChanges,
    pub(crate) control: &'a crate::progress::OperationControl,
    pub(crate) operation_pool: Option<&'a ThreadPool>,
}

pub(crate) fn finalize_repair_result_after_reconstruction(
    request: FinalizeRepairRequest<'_>,
) -> Result<SnapshotRepairResult> {
    let FinalizeRepairRequest {
        snapshot_manifest,
        destination_dir,
        staging_dir,
        remove_added_files,
        runtime_hints,
        repair_plan,
        control,
        operation_pool,
    } = request;
    let _finalize_span = control.profile_span("installation.repair.finalize");
    let mut reconstructed_extra_file_count = 0usize;
    let mut removed_extra_file_count = 0usize;
    let mut preserved_extra_dir = None;

    // Finish the publication sequence without cancellation between entries.
    // I/O failure can still leave a partially repaired destination; retry reclassifies it.
    control.checkpoint()?;
    if !remove_added_files && repair_plan.extra_file_count > 0 {
        preserved_extra_dir = Some(preserve_extra_files(
            destination_dir,
            snapshot_manifest,
            &repair_plan.classification,
        )?);
    }

    // Dependencies staged for link validation are not themselves repair targets.
    let changed_paths: HashSet<_> = repair_plan
        .classification
        .missing_files
        .iter()
        .chain(&repair_plan.classification.corrupt_files)
        .map(|issue| issue.path.as_str())
        .collect();
    let mut publication_manifest = repair_plan.affected_manifest.clone();
    publication_manifest
        .entries
        .retain(|entry| changed_paths.contains(entry.path()));
    promote_repaired_files(
        &publication_manifest,
        destination_dir,
        staging_dir,
        control,
        operation_pool,
    )?;
    if let Some(preserved_extra_dir) = preserved_extra_dir.as_ref() {
        reconstructed_extra_file_count = reconstruct_preserved_extra_files(
            preserved_extra_dir,
            destination_dir,
            snapshot_manifest,
        )?;
        let _ = crate::filesystem::remove_dir_all(preserved_extra_dir);
    }

    if remove_added_files {
        removed_extra_file_count = remove_extra_files(
            destination_dir,
            snapshot_manifest,
            &repair_plan.classification.extra_files,
        )?;
    }

    let mut verification_manifest = repair_plan.affected_manifest.clone();
    if !repair_plan.metadata_updates.is_empty() {
        for entry in &repair_plan.metadata_updates {
            let path = crate::filesystem::paths::resolve_manifest_relative_path(
                destination_dir,
                entry.path(),
            )?;
            match entry {
                SnapshotEntry::File(file) => super::entries::apply_entry_permissions(
                    &path,
                    file.read_only,
                    Some(file.executable),
                )?,
                SnapshotEntry::Directory(directory) => {
                    super::entries::apply_entry_permissions(&path, directory.read_only, None)?
                }
                SnapshotEntry::Symlink(_) => {
                    unreachable!("symlink changes require entry replacement")
                }
            }
        }
        verification_manifest
            .entries
            .extend(repair_plan.metadata_updates.iter().cloned());
    }
    let mut final_classification = if verification_manifest.entries.is_empty() {
        RepairClassification::default()
    } else {
        classify_expected_entries_with_marker_namespace_and_pool(
            VerifyRequest {
                snapshot_manifest: &verification_manifest,
                dir: destination_dir,
                mode: crate::installation::inspect::VerifyMode::Fast,
                runtime_hints,
                control: control.clone(),
            },
            "installation.repair.verify.final",
            operation_pool,
        )?
        .0
    };
    if remove_added_files {
        let path_resolver = crate::filesystem::paths::ManifestPathResolver::new(destination_dir)?;
        for issue in &repair_plan.classification.extra_files {
            let path = path_resolver.resolve(&issue.path)?;
            if fs::symlink_metadata(path).is_ok() {
                final_classification.push(RepairIssue {
                    kind: RepairIssueKind::Extra,
                    path: issue.path.clone(),
                    detail: format!(
                        "extra filesystem entry remains after repair: {}",
                        issue.path
                    ),
                });
            }
        }
    }
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
        crate::filesystem::remove_dir_all(&preserve_dir)?;
    }
    crate::filesystem::create_dir_all(&preserve_dir)?;

    let manifest_paths = manifest_paths_set(snapshot_manifest);

    for issue in &classification.extra_files {
        if manifest_paths.contains(issue.path.as_str()) {
            continue;
        }
        let source =
            crate::filesystem::paths::resolve_manifest_relative_path(destination_dir, &issue.path)?;
        if !source.is_file() {
            continue;
        }
        let dest =
            crate::filesystem::paths::resolve_manifest_relative_path(&preserve_dir, &issue.path)?;
        if let Some(parent) = dest.parent() {
            crate::filesystem::create_dir_all(parent)?;
        }
        fs::copy(&source, &dest)?;
    }

    Ok(preserve_dir)
}

fn promote_repaired_files(
    repaired_manifest: &SnapshotManifest,
    destination_dir: &Path,
    staging_dir: &Path,
    control: &crate::progress::OperationControl,
    operation_pool: Option<&ThreadPool>,
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
    let reopened_ancestors = reopen_read_only_directories(repaired_manifest, destination_dir)?;
    for directory in &directories {
        let dest = crate::filesystem::paths::resolve_manifest_relative_path(
            destination_dir,
            &directory.path,
        )?;
        let is_existing_directory = fs::symlink_metadata(&dest)
            .map(|metadata| metadata.file_type().is_dir() && !metadata.file_type().is_symlink())
            .unwrap_or(false);
        if !is_existing_directory {
            remove_existing_entry(&dest)?;
        }
        crate::filesystem::create_dir_all(&dest)?;
    }

    for entry in &repaired_manifest.entries {
        let path = match entry {
            SnapshotEntry::File(file) => &file.path,
            SnapshotEntry::Symlink(link) => &link.path,
            SnapshotEntry::Directory(_) => continue,
        };
        let source = crate::filesystem::paths::resolve_manifest_relative_path(staging_dir, path)?;
        let dest = crate::filesystem::paths::resolve_manifest_relative_path(destination_dir, path)?;
        if let Some(parent) = dest.parent() {
            crate::filesystem::create_dir_all(parent)?;
        }
        remove_existing_entry(&dest)?;
        if matches!(entry, SnapshotEntry::File(_)) {
            fs::rename(&source, &dest)?;
        }
    }

    // Reconstruction emits bytes. Apply the same file attributes and links as
    // normal reconstruction before declaring the repaired destination verified.
    finalize_snapshot_entries(
        repaired_manifest,
        destination_dir,
        control,
        operation_pool.map_or(1, ThreadPool::current_num_threads),
        operation_pool,
    )?;
    directories.sort_by_key(|directory| std::cmp::Reverse(directory.path.matches('/').count()));
    for directory in directories {
        let dest = crate::filesystem::paths::resolve_manifest_relative_path(
            destination_dir,
            &directory.path,
        )?;
        let mut permissions = fs::metadata(&dest)?.permissions();
        crate::filesystem::set_read_only(&mut permissions, directory.read_only);
        fs::set_permissions(dest, permissions)?;
    }
    for path in reopened_ancestors {
        let mut permissions = fs::metadata(&path)?.permissions();
        crate::filesystem::set_read_only(&mut permissions, true);
        fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

/// Unix forbids adding or removing entries in a read-only directory, so promotion
/// makes the affected existing directories writable first. Repaired directories get
/// their manifest permissions afterwards; the returned unchanged ancestors are
/// restored by the caller. A failed promotion leaves them writable, which a retry
/// reclassifies as a read-only mismatch.
fn reopen_read_only_directories(
    repaired_manifest: &SnapshotManifest,
    destination_dir: &Path,
) -> Result<Vec<PathBuf>> {
    let repaired_directories: HashSet<&str> = repaired_manifest
        .entries
        .iter()
        .filter(|entry| matches!(entry, SnapshotEntry::Directory(_)))
        .map(SnapshotEntry::path)
        .collect();
    let mut directories = std::collections::BTreeSet::new();
    for entry in &repaired_manifest.entries {
        for (offset, _) in entry.path().match_indices('/') {
            directories.insert(&entry.path()[..offset]);
        }
    }
    directories.extend(repaired_directories.iter().copied());
    let mut reopened_ancestors = Vec::new();
    for directory in directories {
        let path =
            crate::filesystem::paths::resolve_manifest_relative_path(destination_dir, directory)?;
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if !metadata.file_type().is_dir() || !metadata.permissions().readonly() {
            continue;
        }
        let mut permissions = metadata.permissions();
        crate::filesystem::set_read_only(&mut permissions, false);
        fs::set_permissions(&path, permissions)?;
        if !repaired_directories.contains(directory) {
            reopened_ancestors.push(path);
        }
    }
    Ok(reopened_ancestors)
}

fn remove_existing_entry(path: &Path) -> Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
        crate::filesystem::remove_dir_all(path)
    } else {
        crate::filesystem::remove_file(path)
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
        if !(entry.file_type().is_file()
            || (entry.file_type().is_symlink() && entry.path().is_file()))
        {
            continue;
        }
        let Ok(relative_path) = entry.path().strip_prefix(preserve_dir) else {
            continue;
        };
        let relative = relative_path.to_string_lossy().replace('\\', "/");
        if manifest_paths.contains(relative.as_str()) {
            continue;
        }
        let dest =
            crate::filesystem::paths::resolve_manifest_relative_path(destination_dir, &relative)?;
        if dest.exists() {
            continue;
        }
        if let Some(parent) = dest.parent() {
            crate::filesystem::create_dir_all(parent)?;
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
        let path =
            crate::filesystem::paths::resolve_manifest_relative_path(destination_dir, &issue.path)?;
        if fs::symlink_metadata(&path).is_ok() {
            remove_existing_entry(&path)?;
            removed += 1;
        }
    }

    Ok(removed)
}

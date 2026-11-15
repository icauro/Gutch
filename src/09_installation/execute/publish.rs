//! Installation publication policies. Selective publication is retryable, not a multi-file atomic commit.
use super::entries::finalize_snapshot_entries;
use crate::Error;
use crate::Result;
use crate::filesystem;
use crate::filesystem::paths::ManifestPathResolver;
use crate::filesystem::set_read_only;
use crate::installation::contracts::SnapshotRepairResult;
use crate::installation::inspect::InspectionSpans;
use crate::installation::inspect::RepairClassification;
use crate::installation::inspect::RepairIssue;
use crate::installation::inspect::VerifyMode;
use crate::installation::inspect::VerifyRequest;
use crate::installation::inspect::classify_expected_entries;
use crate::installation::plan::InstallationChanges;
use crate::manifest::SnapshotEntry;
use crate::manifest::snapshot::SnapshotManifest;
use crate::progress::OperationControl;
use crate::runtime::RuntimeResourceHints;
use rayon::ThreadPool;
use std::cmp::Reverse;
use std::collections::BTreeSet;
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

pub(crate) enum Publication<'a> {
    ReplaceDirectory,
    Selective {
        target: &'a SnapshotManifest,
        changes: &'a InstallationChanges,
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
    pub(crate) remove_extra_files: bool,
    pub(crate) runtime_hints: &'a RuntimeResourceHints,
    pub(crate) repair_plan: &'a InstallationChanges,
    pub(crate) control: &'a OperationControl,
    pub(crate) operation_pool: Option<&'a ThreadPool>,
}

pub(crate) fn finalize_repair_result_after_reconstruction(
    request: FinalizeRepairRequest<'_>,
) -> Result<SnapshotRepairResult> {
    let FinalizeRepairRequest {
        snapshot_manifest,
        destination_dir,
        staging_dir,
        remove_extra_files,
        runtime_hints,
        repair_plan,
        control,
        operation_pool,
    } = request;
    let _finalize_span = control.profile_span("installation.repair.finalize");
    let mut removed_extra_file_count = 0usize;
    let destination = ManifestPathResolver::new(destination_dir)?;

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
    if !remove_extra_files {
        refuse_to_replace_non_empty_directories(&publication_manifest, &destination)?;
    }
    // Finish the publication sequence without cancellation between entries.
    // I/O failure can still leave a partially repaired destination; retry reclassifies it.
    control.checkpoint()?;
    promote_repaired_files(
        &publication_manifest,
        snapshot_manifest,
        destination_dir,
        staging_dir,
        control,
        operation_pool,
    )?;
    if remove_extra_files {
        removed_extra_file_count = delete_extra_files(
            destination_dir,
            snapshot_manifest,
            &repair_plan.classification.extra_files,
        )?;
    }

    let mut verification_manifest = repair_plan.affected_manifest.clone();
    if !repair_plan.metadata_updates.is_empty() {
        for entry in &repair_plan.metadata_updates {
            let path = destination.resolve_entry(entry.path())?;
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
        classify_expected_entries(VerifyRequest {
            snapshot_manifest: &verification_manifest,
            dir: destination_dir,
            mode: VerifyMode::Fast,
            runtime_hints,
            control: control.clone(),
            spans: &InspectionSpans::REPAIR_RESULT,
            pool: operation_pool,
        })?
        .0
    };
    if remove_extra_files {
        for issue in &repair_plan.classification.extra_files {
            let path = destination.resolve_entry(&issue.path)?;
            if fs::symlink_metadata(path).is_ok() {
                final_classification.push(RepairIssue::extra(
                    &issue.path,
                    format!(
                        "extra filesystem entry remains after repair: {}",
                        issue.path
                    ),
                ));
            }
        }
    }
    if !final_classification.missing_files.is_empty()
        || !final_classification.corrupt_files.is_empty()
        || (remove_extra_files && !final_classification.extra_files.is_empty())
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
        reconstruct_result: None,
    })
}

/// Promoting a file or link over a real directory deletes the directory's
/// contents. When extra entries must be kept, refuse before anything changes:
/// they could not be put back at a path that is now a file.
fn refuse_to_replace_non_empty_directories(
    publication_manifest: &SnapshotManifest,
    destination: &ManifestPathResolver,
) -> Result<()> {
    for entry in &publication_manifest.entries {
        if matches!(entry, SnapshotEntry::Directory(_)) {
            continue;
        }
        let path = destination.resolve_entry(entry.path())?;
        let is_real_directory = fs::symlink_metadata(&path).is_ok_and(|metadata| {
            metadata.file_type().is_dir() && !metadata.file_type().is_symlink()
        });
        if is_real_directory && fs::read_dir(&path)?.next().is_some() {
            return Err(Error::invalid_input(format!(
                "repairing '{}' would delete the extra entries inside the directory now at that path;                  move them elsewhere or repair with added files removed",
                entry.path()
            )));
        }
    }
    Ok(())
}

fn promote_repaired_files(
    repaired_manifest: &SnapshotManifest,
    complete_manifest: &SnapshotManifest,
    destination_dir: &Path,
    staging_dir: &Path,
    control: &OperationControl,
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
    let destination = ManifestPathResolver::new(destination_dir)?;
    let staging = ManifestPathResolver::new(staging_dir)?;
    let reopened_ancestors = reopen_read_only_directories(repaired_manifest, &destination)?;
    for directory in &directories {
        let dest = destination.resolve_entry(&directory.path)?;
        let is_existing_directory = fs::symlink_metadata(&dest)
            .map(|metadata| metadata.file_type().is_dir() && !metadata.file_type().is_symlink())
            .unwrap_or(false);
        if !is_existing_directory {
            remove_existing_entry(&dest)?;
        }
        filesystem::create_dir_all(&dest)?;
    }

    for entry in &repaired_manifest.entries {
        let path = match entry {
            SnapshotEntry::File(file) => &file.path,
            SnapshotEntry::Symlink(link) => &link.path,
            SnapshotEntry::Directory(_) => continue,
        };
        let source = staging.resolve_entry(path)?;
        let dest = destination.resolve_entry(path)?;
        if let Some(parent) = dest.parent() {
            filesystem::create_dir_all(parent)?;
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
        complete_manifest,
        destination_dir,
        control,
        operation_pool.map_or(1, ThreadPool::current_num_threads),
        operation_pool,
    )?;
    directories.sort_by_key(|directory| Reverse(directory.path.matches('/').count()));
    for directory in directories {
        let dest = destination.resolve_entry(&directory.path)?;
        let mut permissions = fs::metadata(&dest)?.permissions();
        set_read_only(&mut permissions, directory.read_only);
        fs::set_permissions(dest, permissions)?;
    }
    for path in reopened_ancestors {
        let mut permissions = fs::metadata(&path)?.permissions();
        set_read_only(&mut permissions, true);
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
    destination: &ManifestPathResolver,
) -> Result<Vec<PathBuf>> {
    let repaired_directories: HashSet<&str> = repaired_manifest
        .entries
        .iter()
        .filter(|entry| matches!(entry, SnapshotEntry::Directory(_)))
        .map(SnapshotEntry::path)
        .collect();
    let mut directories = BTreeSet::new();
    for entry in &repaired_manifest.entries {
        for (offset, _) in entry.path().match_indices('/') {
            directories.insert(&entry.path()[..offset]);
        }
    }
    directories.extend(repaired_directories.iter().copied());
    let mut reopened_ancestors = Vec::new();
    for directory in directories {
        let path = destination.resolve_entry(directory)?;
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if !metadata.file_type().is_dir() || !metadata.permissions().readonly() {
            continue;
        }
        let mut permissions = metadata.permissions();
        set_read_only(&mut permissions, false);
        fs::set_permissions(&path, permissions)?;
        if !repaired_directories.contains(directory) {
            reopened_ancestors.push(path);
        }
    }
    Ok(reopened_ancestors)
}

fn remove_existing_entry(path: &Path) -> Result<()> {
    let Some(metadata) = filesystem::symlink_metadata_if_exists(path)? else {
        return Ok(());
    };
    if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
        filesystem::remove_dir_all(path)
    } else {
        filesystem::remove_file(path)
    }
}

fn delete_extra_files(
    destination_dir: &Path,
    snapshot_manifest: &SnapshotManifest,
    extra_files: &[RepairIssue],
) -> Result<usize> {
    let manifest_paths = manifest_paths_set(snapshot_manifest);
    let destination = ManifestPathResolver::new(destination_dir)?;
    let mut removed = 0usize;

    let mut paths = extra_files.iter().collect::<Vec<_>>();
    paths.sort_by_key(|issue| Reverse(issue.path.matches('/').count()));
    for issue in paths {
        if manifest_paths.contains(issue.path.as_str()) {
            continue;
        }
        let path = destination.resolve_entry(&issue.path)?;
        if fs::symlink_metadata(&path).is_ok() {
            remove_existing_entry(&path)?;
            removed += 1;
        }
    }

    Ok(removed)
}

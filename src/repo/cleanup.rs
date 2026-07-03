use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::Result;

use super::{
    load_chunk_index, load_gutch_repo_manifest, load_target_manifest, required_bundle_ids,
    target_manifest_bin_path, target_manifest_path,
};

pub fn remove_target_from_repo(repo_root: &Path, target_id: &str) -> Result<()> {
    let (repo_paths, _) = load_gutch_repo_manifest(repo_root)?;
    let manifest_path = target_manifest_path(&repo_paths, target_id);
    let manifest_bin_path = target_manifest_bin_path(&repo_paths, target_id);
    for path in [&manifest_path, &manifest_bin_path] {
        if !path.exists() {
            continue;
        }
        if path.is_dir() {
            std::fs::remove_dir_all(path)?;
        } else {
            std::fs::remove_file(path)?;
        }
    }
    Ok(())
}

pub fn cleanup_orphan_bundles_in_repo(repo_root: &Path) -> Result<usize> {
    cleanup_orphan_bundles_in_repo_with_progress(repo_root, &None)
}

pub fn cleanup_orphan_bundles_in_repo_with_progress(
    repo_root: &Path,
    progress: &Option<Arc<dyn crate::progress::GutchProgressSink>>,
) -> Result<usize> {
    use crate::progress::{GutchOperation, GutchProgressEvent, emit_progress};

    const TOTAL: usize = 1000;
    const SCAN_TARGETS_END: usize = 50;
    const LOAD_MANIFESTS_END: usize = 400;
    const SCAN_BUNDLES_END: usize = 900;

    emit_progress(
        progress,
        GutchProgressEvent {
            operation: GutchOperation::TargetDelete,
            phase: "scan_targets".to_string(),
            message: "Scanning remaining targets".to_string(),
            current: 0,
            total: TOTAL,
            transferred_bytes: 0,
            total_bytes: 0,
            detail: None,
            done: false,
        },
    );
    let (repo_paths, _) = load_gutch_repo_manifest(repo_root)?;
    let remaining_targets: Vec<String> = if repo_paths.targets_root.exists() {
        let mut ids: Vec<String> = std::fs::read_dir(&repo_paths.targets_root)?
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry.path().is_file()
                    && entry.path().extension().and_then(|value| value.to_str()) == Some("json")
            })
            .filter_map(|entry| {
                entry
                    .path()
                    .file_stem()
                    .and_then(|value| value.to_str())
                    .map(str::to_string)
            })
            .collect();
        ids.sort();
        ids
    } else {
        Vec::new()
    };

    let chunk_index = load_chunk_index(&repo_paths)?;
    let mut still_required_bundle_ids = HashSet::new();
    let target_count = remaining_targets.len().max(1);
    for (i, target_id) in remaining_targets.iter().enumerate() {
        let progress_current =
            SCAN_TARGETS_END + (LOAD_MANIFESTS_END - SCAN_TARGETS_END) * i / target_count;
        emit_progress(
            progress,
            GutchProgressEvent {
                operation: GutchOperation::TargetDelete,
                phase: "load_manifests".to_string(),
                message: format!(
                    "Loading target manifests ({}/{})",
                    i + 1,
                    remaining_targets.len()
                ),
                current: progress_current,
                total: TOTAL,
                transferred_bytes: 0,
                total_bytes: 0,
                detail: Some(target_id.clone()),
                done: false,
            },
        );
        let manifest = load_target_manifest(&repo_paths, target_id)?;
        still_required_bundle_ids.extend(required_bundle_ids(&manifest, &chunk_index));
    }

    let bundle_files: Vec<PathBuf> = if repo_paths.bundles_root.exists() {
        walkdir::WalkDir::new(&repo_paths.bundles_root)
            .sort_by_file_name()
            .into_iter()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.path().is_file())
            .filter(|entry| crate::bundle_id_from_bundle_path(entry.path()).is_some())
            .map(|entry| entry.path().to_path_buf())
            .collect()
    } else {
        Vec::new()
    };

    let bundle_count = bundle_files.len().max(1);
    let mut orphan_bundles = Vec::new();
    for (i, bundle_path) in bundle_files.iter().enumerate() {
        if i % 50 == 0 || i == bundle_files.len() - 1 {
            let progress_current =
                SCAN_BUNDLES_END + (SCAN_BUNDLES_END - LOAD_MANIFESTS_END) * i / bundle_count;
            emit_progress(
                progress,
                GutchProgressEvent {
                    operation: GutchOperation::TargetDelete,
                    phase: "scan_bundles".to_string(),
                    message: format!("Scanning bundles ({}/{})", i + 1, bundle_files.len()),
                    current: progress_current.min(SCAN_BUNDLES_END),
                    total: TOTAL,
                    transferred_bytes: 0,
                    total_bytes: 0,
                    detail: None,
                    done: false,
                },
            );
        }
        if let Some(bundle_id) = crate::bundle_id_from_bundle_path(bundle_path)
            && !still_required_bundle_ids.contains(&bundle_id)
        {
            orphan_bundles.push(bundle_path.clone());
        }
    }

    emit_progress(
        progress,
        GutchProgressEvent {
            operation: GutchOperation::TargetDelete,
            phase: "delete_orphans".to_string(),
            message: format!("Removing {} orphan bundle(s)", orphan_bundles.len()),
            current: SCAN_BUNDLES_END,
            total: TOTAL,
            transferred_bytes: 0,
            total_bytes: 0,
            detail: None,
            done: false,
        },
    );
    for orphan_path in &orphan_bundles {
        std::fs::remove_file(orphan_path)?;
    }
    Ok(orphan_bundles.len())
}

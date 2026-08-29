use std::collections::HashSet;
use std::path::Path;

use crate::Result;

use super::inventory::{list_repo_bundle_paths, list_repo_snapshot_ids};
use super::{
    load_chunk_index, load_gutch_repo_manifest, load_snapshot_manifest, required_bundle_ids,
    snapshot_metadata_path, validate_snapshot_id,
};

pub(crate) fn remove_snapshot_from_repo(repo_root: &Path, snapshot_id: &str) -> Result<()> {
    validate_snapshot_id(snapshot_id)?;
    let (repo_paths, _) = load_gutch_repo_manifest(repo_root)?;
    let manifest_path = snapshot_metadata_path(&repo_paths, snapshot_id)?;
    let Some(snapshot_dir) = manifest_path.parent() else {
        return Ok(());
    };
    if !snapshot_dir.exists() {
        return Ok(());
    }
    let staged_snapshot_dir = repo_paths
        .repo_root
        .join(format!(".deleting-snapshot-{snapshot_id}"));
    if staged_snapshot_dir.exists() {
        return Err(crate::Error::manifest(format!(
            "staged snapshot deletion already exists at {}",
            staged_snapshot_dir.display()
        )));
    }
    std::fs::rename(snapshot_dir, &staged_snapshot_dir)?;
    std::fs::remove_dir_all(staged_snapshot_dir)?;
    Ok(())
}

pub(crate) fn cleanup_orphan_bundles_in_repo(
    repo_root: &Path,
    control: &crate::GutchOperationControl,
) -> Result<usize> {
    use crate::progress::{GutchOperation, GutchProgressEvent, GutchProgressPhase, emit_progress};

    const TOTAL: usize = 1000;
    const SCAN_SNAPSHOTS_END: usize = 50;
    const LOAD_MANIFESTS_END: usize = 400;
    const SCAN_BUNDLES_END: usize = 900;

    emit_progress(
        control.progress(),
        GutchProgressEvent {
            operation: GutchOperation::SnapshotDelete,
            phase: GutchProgressPhase::ScanSnapshots,
            message: "Scanning remaining snapshots".to_string(),
            current: 0,
            total: TOTAL,
            transferred_bytes: 0,
            total_bytes: 0,
            detail: None,
            done: false,
        },
    );
    let (repo_paths, _) = load_gutch_repo_manifest(repo_root)?;
    let remaining_snapshots = list_repo_snapshot_ids(&repo_paths)?;

    let chunk_index = load_chunk_index(&repo_paths)?;
    let mut still_required_bundle_ids = HashSet::new();
    let snapshot_count = remaining_snapshots.len().max(1);
    for (i, snapshot_id) in remaining_snapshots.iter().enumerate() {
        control.checkpoint()?;
        let progress_current =
            SCAN_SNAPSHOTS_END + (LOAD_MANIFESTS_END - SCAN_SNAPSHOTS_END) * i / snapshot_count;
        emit_progress(
            control.progress(),
            GutchProgressEvent {
                operation: GutchOperation::SnapshotDelete,
                phase: GutchProgressPhase::LoadManifests,
                message: format!(
                    "Loading snapshot manifests ({}/{})",
                    i + 1,
                    remaining_snapshots.len()
                ),
                current: progress_current,
                total: TOTAL,
                transferred_bytes: 0,
                total_bytes: 0,
                detail: Some(snapshot_id.clone()),
                done: false,
            },
        );
        let manifest = load_snapshot_manifest(&repo_paths, snapshot_id)?;
        still_required_bundle_ids.extend(required_bundle_ids(&manifest, &chunk_index));
    }

    let bundle_files = list_repo_bundle_paths(&repo_paths);

    let bundle_count = bundle_files.len().max(1);
    let mut orphan_bundles = Vec::new();
    for (i, bundle_path) in bundle_files.iter().enumerate() {
        control.checkpoint()?;
        if i % 50 == 0 || i == bundle_files.len() - 1 {
            let progress_current =
                SCAN_BUNDLES_END + (SCAN_BUNDLES_END - LOAD_MANIFESTS_END) * i / bundle_count;
            emit_progress(
                control.progress(),
                GutchProgressEvent {
                    operation: GutchOperation::SnapshotDelete,
                    phase: GutchProgressPhase::ScanBundles,
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
        control.progress(),
        GutchProgressEvent {
            operation: GutchOperation::SnapshotDelete,
            phase: GutchProgressPhase::DeleteOrphans,
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
        control.checkpoint()?;
        std::fs::remove_file(orphan_path)?;
    }
    Ok(orphan_bundles.len())
}

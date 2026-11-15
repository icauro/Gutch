//! Snapshot removal and orphan artifact cleanup.

use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;

use crate::Result;

use super::inventory::list_repo_artifact_paths;
use super::inventory::list_repo_snapshot_ids;
use crate::progress::OperationControl;
use crate::repo::artifacts::required_artifacts;
use crate::repo::index::load_chunk_index;
use crate::repo::paths::load_gutch_repo_manifest;
use crate::repo::snapshot_manifests::load_snapshot_manifest;
use crate::repo::snapshot_manifests::snapshot_metadata_path;
use crate::repo::snapshot_manifests::validate_snapshot_id;
use std::fs;

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
    // Remove in place: a partly removed directory is simply removed again on
    // retry, where a staged rename would block every later attempt.
    fs::remove_dir_all(snapshot_dir).map_err(|error| {
        crate::Error::io_context("removing snapshot directory", snapshot_dir, error)
    })?;
    Ok(())
}

pub(crate) fn cleanup_orphan_artifacts_in_repo(
    repo_root: &Path,
    control: &OperationControl,
) -> Result<usize> {
    let orphan_paths = plan_orphan_artifact_cleanup(repo_root, None, control)?;
    remove_orphan_artifacts(&orphan_paths, control)
}

pub(crate) fn plan_orphan_artifact_cleanup(
    repo_root: &Path,
    excluded_snapshot: Option<&str>,
    control: &OperationControl,
) -> Result<Vec<PathBuf>> {
    use crate::progress::OperationKind;
    use crate::progress::ProgressEvent;
    use crate::progress::ProgressPhase;
    use crate::progress::emit_progress;

    const TOTAL: usize = 1000;
    const SCAN_SNAPSHOTS_END: usize = 50;
    const LOAD_MANIFESTS_END: usize = 400;
    const SCAN_BUNDLES_END: usize = 900;

    emit_progress(
        control.progress(),
        ProgressEvent::new(
            OperationKind::SnapshotDelete,
            ProgressPhase::ScanSnapshots,
            "Scanning remaining snapshots",
            0,
            TOTAL,
        ),
    );
    let (repo_paths, _) = load_gutch_repo_manifest(repo_root)?;
    let remaining_snapshots = list_repo_snapshot_ids(&repo_paths)?
        .into_iter()
        .filter(|id| Some(id.as_str()) != excluded_snapshot)
        .collect::<Vec<_>>();

    let chunk_index = load_chunk_index(&repo_paths)?;
    let mut still_required_artifacts = HashSet::new();
    let snapshot_count = remaining_snapshots.len().max(1);
    for (i, snapshot_id) in remaining_snapshots.iter().enumerate() {
        control.checkpoint()?;
        let progress_current =
            SCAN_SNAPSHOTS_END + (LOAD_MANIFESTS_END - SCAN_SNAPSHOTS_END) * i / snapshot_count;
        emit_progress(
            control.progress(),
            ProgressEvent {
                detail: Some(snapshot_id.clone()),
                ..ProgressEvent::new(
                    OperationKind::SnapshotDelete,
                    ProgressPhase::LoadManifests,
                    format!(
                        "Loading snapshot manifests ({}/{})",
                        i + 1,
                        remaining_snapshots.len()
                    ),
                    progress_current,
                    TOTAL,
                )
            },
        );
        let manifest = load_snapshot_manifest(&repo_paths, snapshot_id)?;
        let required_artifacts = required_artifacts(&manifest, &chunk_index)?;
        for artifact in &required_artifacts {
            let path = artifact.local_path(&repo_paths)?;
            if !path.is_file() {
                return Err(crate::Error::repository(format!(
                    "cannot clean repository: snapshot '{snapshot_id}' requires missing artifact '{}'",
                    path.display()
                )));
            }
        }
        still_required_artifacts.extend(required_artifacts);
    }

    let artifact_files = list_repo_artifact_paths(&repo_paths)?;

    let artifact_count = artifact_files.len().max(1);
    let mut orphan_artifacts = Vec::new();
    for (i, (artifact, artifact_path)) in artifact_files.iter().enumerate() {
        control.checkpoint()?;
        if i % 50 == 0 || i == artifact_files.len() - 1 {
            let progress_current =
                LOAD_MANIFESTS_END + (SCAN_BUNDLES_END - LOAD_MANIFESTS_END) * i / artifact_count;
            emit_progress(
                control.progress(),
                ProgressEvent::new(
                    OperationKind::SnapshotDelete,
                    ProgressPhase::ScanArtifacts,
                    format!("Scanning artifacts ({}/{})", i + 1, artifact_files.len()),
                    progress_current.min(SCAN_BUNDLES_END),
                    TOTAL,
                ),
            );
        }
        if !still_required_artifacts.contains(artifact) {
            orphan_artifacts.push(artifact_path.clone());
        }
    }

    emit_progress(
        control.progress(),
        ProgressEvent::new(
            OperationKind::SnapshotDelete,
            ProgressPhase::DeleteOrphans,
            format!("Removing {} orphan artifact(s)", orphan_artifacts.len()),
            SCAN_BUNDLES_END,
            TOTAL,
        ),
    );
    Ok(orphan_artifacts)
}

pub(crate) fn remove_orphan_artifacts(
    orphan_artifacts: &[PathBuf],
    control: &OperationControl,
) -> Result<usize> {
    for orphan_path in orphan_artifacts {
        control.checkpoint()?;
        if !super::coordination::defer_artifact_deletion(orphan_path)? {
            fs::remove_file(orphan_path)?;
        }
    }
    Ok(orphan_artifacts.len())
}

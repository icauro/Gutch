use std::path::Path;

use serde::Deserialize;
use serde::Serialize;

use crate::manifest::SnapshotTagSet;
use crate::progress;
use crate::progress::GutchOperation;
use crate::progress::GutchProgressEvent;
use crate::progress::GutchProgressPhase;
use crate::repo::Result;
use crate::repo::remove_snapshot_from_repo;

/// Result of explicitly retagging one exact snapshot ID.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RetagSnapshotResult {
    pub snapshot_id: String,
    pub previous_tags: SnapshotTagSet,
    pub new_tags: SnapshotTagSet,
}

pub fn delete_repo_snapshot_and_cleanup(
    repo_root: &Path,
    snapshot_id: &str,
    control: &crate::progress::GutchOperationControl,
) -> Result<usize> {
    let operation_root = (repo_root).to_path_buf();
    crate::repo::coordination::write(&operation_root, || {
        control.checkpoint()?;
        // Total matches cleanup_orphan_artifacts_in_repo.
        const TOTAL: usize = 1000;

        progress::emit_progress(
            control.progress(),
            GutchProgressEvent {
                operation: GutchOperation::SnapshotDelete,
                phase: GutchProgressPhase::RemoveManifest,
                message: format!("Removing snapshot '{}'", snapshot_id),
                current: 0,
                total: TOTAL,
                transferred_bytes: 0,
                total_bytes: 0,
                detail: Some(snapshot_id.to_string()),
                done: false,
            },
        );
        let orphan_paths =
            super::cleanup::plan_orphan_artifact_cleanup(repo_root, Some(snapshot_id), control)?;
        control.checkpoint()?;
        remove_snapshot_from_repo(repo_root, snapshot_id)?;
        let orphan_count = super::cleanup::remove_orphan_artifacts(&orphan_paths, control)?;

        progress::emit_progress(
            control.progress(),
            GutchProgressEvent {
                operation: GutchOperation::SnapshotDelete,
                phase: GutchProgressPhase::Done,
                message: format!(
                    "Deleted snapshot, removed {} orphan bundle(s)",
                    orphan_count
                ),
                current: TOTAL,
                total: TOTAL,
                transferred_bytes: 0,
                total_bytes: 0,
                detail: None,
                done: true,
            },
        );
        Ok(orphan_count)
    })
}

/// Retags one exact snapshot ID. The caller selects the target; tags are
/// never inferred from other snapshots. Reports the previous and new tags.
pub fn retag_snapshot_by_id(
    repo_root: &Path,
    snapshot_id: &str,
    new_tags: SnapshotTagSet,
) -> Result<RetagSnapshotResult> {
    let new_tags = SnapshotTagSet::from_compact_strings(&new_tags.to_canonical_strings())?;
    let operation_root = (repo_root).to_path_buf();
    crate::repo::coordination::write(&operation_root, || {
        let (repo_paths, _) = crate::repo::paths::load_gutch_repo_manifest(repo_root)?;
        let mut manifest =
            crate::repo::snapshot_manifests::load_snapshot_manifest(&repo_paths, snapshot_id)?;
        let previous_tags = manifest.snapshot_tags.clone();
        manifest.snapshot_tags = new_tags.clone();
        crate::repo::snapshot_manifests::write_snapshot_manifest_artifacts(&repo_paths, &manifest)?;
        Ok(RetagSnapshotResult {
            snapshot_id: snapshot_id.to_string(),
            previous_tags,
            new_tags,
        })
    })
}

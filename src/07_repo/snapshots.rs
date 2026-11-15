use std::path::Path;

use serde::Deserialize;
use serde::Serialize;

use crate::manifest::SnapshotTagSet;
use crate::progress;
use crate::progress::OperationKind;
use crate::progress::ProgressEvent;
use crate::progress::ProgressPhase;
use crate::repo::Result;
use crate::repo::cleanup::remove_snapshot_from_repo;

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
    control: &crate::progress::OperationControl,
) -> Result<usize> {
    let operation_root = (repo_root).to_path_buf();
    crate::repo::coordination::write(&operation_root, || {
        control.checkpoint()?;
        // Total matches cleanup_orphan_artifacts_in_repo.
        const TOTAL: usize = 1000;

        progress::emit_progress(
            control.progress(),
            ProgressEvent {
                feedback: None,
                operation: OperationKind::SnapshotDelete,
                phase: ProgressPhase::RemoveManifest,
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
            ProgressEvent {
                feedback: None,
                operation: OperationKind::SnapshotDelete,
                phase: ProgressPhase::Done,
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
    let replacements =
        std::collections::BTreeMap::from([(snapshot_id.to_owned(), new_tags.normalized()?)]);
    Ok(retag_snapshots(repo_root, replacements)?.remove(0))
}

/// Preview complete replacement tag sets for selected snapshots. Untouched
/// snapshots also participate in parity validation.
pub fn plan_snapshot_tag_updates(
    repo_root: &Path,
    replacements: &std::collections::BTreeMap<String, SnapshotTagSet>,
) -> Result<Vec<RetagSnapshotResult>> {
    crate::repo::coordination::read(repo_root, || {
        let (paths, repository) = crate::repo::paths::load_gutch_repo_manifest(repo_root)?;
        let mut results = Vec::new();
        let mut normalized = std::collections::BTreeMap::new();
        for (id, tags) in replacements {
            let manifest = crate::repo::snapshot_manifests::load_snapshot_manifest(&paths, id)?;
            let tags = tags.normalized()?;
            normalized.insert(id.clone(), tags.clone());
            results.push(RetagSnapshotResult {
                snapshot_id: id.clone(),
                previous_tags: manifest.snapshot_tags,
                new_tags: tags,
            });
        }
        super::updates::validate_proposed_snapshot_tags(
            repo_root,
            &repository.constraints.unwrap_or_default(),
            &normalized,
        )?;
        Ok(results)
    })
}

/// Replaces multiple snapshot tag sets in one recoverable repository transaction.
/// A failing rule or write restores every snapshot's previous metadata.
pub fn retag_snapshots(
    repo_root: &Path,
    replacements: std::collections::BTreeMap<String, SnapshotTagSet>,
) -> Result<Vec<RetagSnapshotResult>> {
    crate::repo::coordination::write(repo_root, || {
        let changes = plan_snapshot_tag_updates(repo_root, &replacements)?;
        let (paths, _) = crate::repo::paths::load_gutch_repo_manifest(repo_root)?;
        for change in &changes {
            let mut manifest = crate::repo::snapshot_manifests::load_snapshot_manifest(
                &paths,
                &change.snapshot_id,
            )?;
            manifest.snapshot_tags = change.new_tags.clone();
            crate::repo::snapshot_manifests::write_snapshot_manifest_artifacts(&paths, &manifest)?;
        }
        Ok(changes)
    })
}

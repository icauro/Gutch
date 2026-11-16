//! Snapshot deletion and retagging.

use std::path::Path;

use serde::Deserialize;
use serde::Serialize;

use crate::manifest::SnapshotTagSet;
use crate::progress;
use crate::progress::OperationControl;
use crate::progress::OperationKind;
use crate::progress::ProgressEvent;
use crate::progress::ProgressPhase;
use crate::repo::Result;
use crate::repo::cleanup::remove_snapshot_from_repo;
use crate::repo::coordination;
use crate::repo::paths::load_repo_manifest;
use crate::repo::snapshot_manifests::load_snapshot_manifest;
use crate::repo::snapshot_manifests::snapshot_metadata_path;
use crate::repo::snapshot_manifests::validate_snapshot_id;
use crate::repo::snapshot_manifests::write_snapshot_manifest_artifacts;
use std::collections::BTreeMap;

/// Result of deleting one snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotDeleteResult {
    pub snapshot_id: String,
    /// Artifacts no remaining snapshot used, removed with the snapshot.
    pub removed_artifact_count: usize,
}

/// Result of explicitly retagging one exact snapshot ID.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RetagSnapshotResult {
    pub snapshot_id: String,
    pub previous_tags: SnapshotTagSet,
    pub new_tags: SnapshotTagSet,
}

pub fn delete_snapshot(
    repo_root: &Path,
    snapshot_id: &str,
    control: &OperationControl,
) -> Result<SnapshotDeleteResult> {
    coordination::write(repo_root, || {
        control.checkpoint()?;
        validate_snapshot_id(snapshot_id)?;
        let (repo_paths, _) = load_repo_manifest(repo_root)?;
        // Checked inside the write transaction so a concurrent delete cannot race it.
        // The directory, not its metadata file, marks a partly deleted snapshot.
        let metadata_path = snapshot_metadata_path(&repo_paths, snapshot_id)?;
        if !metadata_path.parent().is_some_and(Path::exists) {
            return Err(crate::Error::repo(format!(
                "snapshot not found in repo: {snapshot_id}"
            )));
        }
        // Total matches plan_orphan_artifact_cleanup.
        const TOTAL: usize = 1000;

        progress::emit_progress(
            control.progress(),
            ProgressEvent {
                detail: Some(snapshot_id.to_string()),
                ..ProgressEvent::new(
                    OperationKind::SnapshotDelete,
                    ProgressPhase::RemoveManifest,
                    format!("Removing snapshot '{}'", snapshot_id),
                    0,
                    TOTAL,
                )
            },
        );
        let orphan_paths =
            super::cleanup::plan_orphan_artifact_cleanup(repo_root, Some(snapshot_id), control)?;
        control.checkpoint()?;
        remove_snapshot_from_repo(repo_root, snapshot_id)?;
        let orphan_count = super::cleanup::remove_orphan_artifacts(&orphan_paths, control)?;

        progress::emit_progress(
            control.progress(),
            ProgressEvent::finished(
                OperationKind::SnapshotDelete,
                format!(
                    "Deleted snapshot, removed {} orphan artifact(s)",
                    orphan_count
                ),
                TOTAL,
                TOTAL,
            ),
        );
        Ok(SnapshotDeleteResult {
            snapshot_id: snapshot_id.to_owned(),
            removed_artifact_count: orphan_count,
        })
    })
}

/// Preview complete replacement tag sets for selected snapshots. Untouched
/// snapshots also participate in parity validation.
pub fn plan_snapshot_tag_updates(
    repo_root: &Path,
    replacements: &BTreeMap<String, SnapshotTagSet>,
) -> Result<Vec<RetagSnapshotResult>> {
    coordination::read(repo_root, || {
        let (paths, repository) = load_repo_manifest(repo_root)?;
        let mut results = Vec::new();
        let mut normalized = BTreeMap::new();
        for (id, tags) in replacements {
            let manifest = load_snapshot_manifest(&paths, id)?;
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
    replacements: BTreeMap<String, SnapshotTagSet>,
    control: &OperationControl,
) -> Result<Vec<RetagSnapshotResult>> {
    coordination::write(repo_root, || {
        let changes = plan_snapshot_tag_updates(repo_root, &replacements)?;
        let (paths, _) = load_repo_manifest(repo_root)?;
        for change in &changes {
            let mut manifest = load_snapshot_manifest(&paths, &change.snapshot_id)?;
            manifest.snapshot_tags = change.new_tags.clone();
            write_snapshot_manifest_artifacts(&paths, &manifest, control)?;
        }
        Ok(changes)
    })
}

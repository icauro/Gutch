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
use std::path::PathBuf;

/// One snapshot to delete.
#[derive(Debug, Clone)]
pub struct SnapshotDeleteRequest {
    pub repo_root: PathBuf,
    pub snapshot_id: String,
    pub control: OperationControl,
}

/// Result of deleting one snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotDeleteResult {
    pub snapshot_id: String,
    /// Artifacts no remaining snapshot used, removed with the snapshot.
    pub removed_artifact_count: usize,
}

/// Complete replacement tag sets, keyed by exact snapshot ID.
#[derive(Debug, Clone)]
pub struct SnapshotTagsReplaceRequest {
    pub repo_root: PathBuf,
    pub replacements: BTreeMap<String, SnapshotTagSet>,
    pub control: OperationControl,
}

/// The tags of one snapshot before and after a replacement.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotTagsChange {
    pub snapshot_id: String,
    pub previous_tags: SnapshotTagSet,
    pub new_tags: SnapshotTagSet,
}

/// The validated changes a tag replacement would make.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SnapshotTagsReplacePlan {
    pub changes: Vec<SnapshotTagsChange>,
}

/// The changes a tag replacement made.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SnapshotTagsReplaceResult {
    pub changes: Vec<SnapshotTagsChange>,
}

pub fn delete_snapshot(request: &SnapshotDeleteRequest) -> Result<SnapshotDeleteResult> {
    let (repo_root, snapshot_id, control) = (
        request.repo_root.as_path(),
        request.snapshot_id.as_str(),
        &request.control,
    );
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
pub fn plan_replace_snapshot_tags(
    request: &SnapshotTagsReplaceRequest,
) -> Result<SnapshotTagsReplacePlan> {
    let repo_root = request.repo_root.as_path();
    request.control.checkpoint()?;
    coordination::read(repo_root, || {
        let (paths, repository) = load_repo_manifest(repo_root)?;
        let mut results = Vec::new();
        let mut normalized = BTreeMap::new();
        for (id, tags) in &request.replacements {
            let manifest = load_snapshot_manifest(&paths, id)?;
            let tags = tags.normalized()?;
            normalized.insert(id.clone(), tags.clone());
            results.push(SnapshotTagsChange {
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
        Ok(SnapshotTagsReplacePlan { changes: results })
    })
}

/// Replaces multiple snapshot tag sets in one recoverable repository transaction.
/// A failing rule or write restores every snapshot's previous metadata.
pub fn replace_snapshot_tags(
    request: &SnapshotTagsReplaceRequest,
) -> Result<SnapshotTagsReplaceResult> {
    let repo_root = request.repo_root.as_path();
    coordination::write(repo_root, || {
        let SnapshotTagsReplacePlan { changes } = plan_replace_snapshot_tags(request)?;
        let (paths, _) = load_repo_manifest(repo_root)?;
        for change in &changes {
            let mut manifest = load_snapshot_manifest(&paths, &change.snapshot_id)?;
            manifest.snapshot_tags = change.new_tags.clone();
            write_snapshot_manifest_artifacts(&paths, &manifest, &request.control)?;
        }
        Ok(SnapshotTagsReplaceResult { changes })
    })
}

//! Streamed installation change previews. Execution always reinspects its inputs.
use super::operation::{ExtraFilePolicy, InstallationPublication};
use super::types::SnapshotReconstructFromRepoRequest;
use crate::{Error, Result};
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallationInspection {
    Content,
    MetadataOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum InstallationAction {
    Keep,
    Add,
    Replace,
    Remove,
    PreserveExtra,
    UpdateMetadata,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum InstallationPlanEvent {
    Entry {
        path: String,
        action: InstallationAction,
        reason: String,
        content_verified: bool,
    },
    Artifact {
        id: String,
        available_locally: bool,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallationPlanSummary {
    pub snapshot_id: String,
    pub content_inspected: bool,
    pub entry_count: usize,
    pub required_artifact_count: usize,
    pub retained_file_bytes: u64,
    pub source_reuse_bytes: u64,
    pub staging_file_bytes: u64,
    /// Logical bytes only; filesystem allocation and backup overhead are not included.
    pub staging_space_is_estimate: bool,
}

/// Inspect without changing the installation. Target entries, extras, and artifacts are each delivered in path/ID order.
/// This is a preview, not a persisted authorization to execute a stale plan.
pub fn visit_installation_plan(
    request: &SnapshotReconstructFromRepoRequest,
    publication: InstallationPublication,
    extra_files: ExtraFilePolicy,
    inspection: InstallationInspection,
    visitor: &mut dyn FnMut(InstallationPlanEvent) -> Result<()>,
) -> Result<InstallationPlanSummary> {
    if publication == InstallationPublication::ReplaceDirectory
        && extra_files == ExtraFilePolicy::Preserve
    {
        return Err(Error::manifest(
            "complete replacement requires removing extra files; use selective publication to preserve them",
        ));
    }
    crate::repo::coordination::read_with_control(&request.repo_path, &request.control, || {
        request.control.checkpoint()?;
        super::apply::validate_installation_paths(
            &request.destination_dir,
            request.staging_dir.as_deref(),
            &request.repo_path,
        )?;
        let (paths, target) = super::snapshot_repo::load_repo_snapshot_manifest(
            &request.repo_path,
            &request.snapshot_id,
        )?;
        let index = crate::repo::index::load_chunk_index(&paths)?;
        let verified = inspection == InstallationInspection::Content;
        let changes = super::plan::inspect_installation_changes(
            &target,
            Some(&index),
            &request.destination_dir,
            if verified {
                super::inspect::VerifyMode::Fast
            } else {
                super::inspect::VerifyMode::MetadataOnly
            },
            &request.runtime_hints,
            &request.control,
            None,
        )?;
        let classification = &changes.classification;
        let metadata_paths: HashSet<_> =
            changes.metadata_updates.iter().map(|entry| entry.path()).collect();
        let missing: HashSet<_> = classification
            .missing_files
            .iter()
            .map(|issue| issue.path.as_str())
            .collect();
        let corrupt: BTreeMap<_, _> = classification
            .corrupt_files
            .iter()
            .map(|issue| (issue.path.as_str(), issue.detail.as_str()))
            .collect();
        let selective = publication == InstallationPublication::Selective;
        let mut retained_file_bytes = 0;
        let entries: BTreeMap<_, _> = target
            .entries
            .iter()
            .map(|entry| (entry.path(), entry))
            .collect();
        for (path, entry) in entries {
            request.control.checkpoint()?;
            let (action, reason) = if missing.contains(path) {
                (InstallationAction::Add, "missing from destination")
            } else if let Some(reason) = corrupt.get(path) {
                (InstallationAction::Replace, *reason)
            } else if selective && metadata_paths.contains(path) {
                retained_file_bytes += entry.as_file().map_or(0, |file| file.size);
                (
                    InstallationAction::UpdateMetadata,
                    "content matches; metadata differs",
                )
            } else if selective {
                retained_file_bytes += entry.as_file().map_or(0, |file| file.size);
                (
                    InstallationAction::Keep,
                    if verified {
                        "matches target"
                    } else {
                        "metadata matches; content unverified"
                    },
                )
            } else {
                (InstallationAction::Replace, "complete staged replacement")
            };
            visitor(InstallationPlanEvent::Entry {
                path: path.to_owned(),
                action,
                reason: reason.to_owned(),
                content_verified: verified
                    && !missing.contains(path)
                    && !corrupt.contains_key(path),
            })?;
        }
        let mut extras: Vec<_> = classification.extra_files.iter().collect();
        extras.sort_by(|left, right| left.path.cmp(&right.path));
        for extra in extras {
            request.control.checkpoint()?;
            visitor(InstallationPlanEvent::Entry {
                path: extra.path.clone(),
                action: if extra_files == ExtraFilePolicy::Preserve {
                    InstallationAction::PreserveExtra
                } else {
                    InstallationAction::Remove
                },
                reason: "absent from target snapshot".into(),
                content_verified: false,
            })?;
        }
        let execution_manifest = if selective {
            &changes.affected_manifest
        } else {
            &target
        };
        let source_manifest = request
            .source_snapshot_id
            .as_ref()
            .map(|id| crate::repo::snapshot_manifests::load_snapshot_manifest(&paths, id))
            .transpose()?;
        let source_directory = request
            .source_dir
            .as_deref()
            .unwrap_or(&request.destination_dir);
        let source = if verified {
            super::apply::resolve_verified_source(
                source_manifest
                    .as_ref()
                    .map(|manifest| super::types::SnapshotReconstructSource {
                        snapshot_manifest: manifest,
                        destination_dir: source_directory,
                    }),
                &request.runtime_hints,
                &request.control,
            )?
        } else {
            None
        };
        let artifacts = crate::repo::artifacts::required_artifacts(execution_manifest, &index)?;
        let existing = artifacts
            .iter()
            .filter(|artifact| artifact.local_path(&paths).is_ok_and(|path| path.is_file()))
            .cloned()
            .collect();
        let jobs = super::plan::build_reconstruct_pipeline_plan(
            execution_manifest,
            &index,
            &paths,
            &existing,
            source.as_ref().map(|source| source.source()),
        )?;
        let source_reuse_bytes = jobs
            .jobs
            .iter()
            .map(|job| match job {
                super::jobs::ReconstructJob::SourceChunk(job) => {
                    u64::from(job.raw_size) * job.targets.len() as u64
                }
                super::jobs::ReconstructJob::CopySourceFile { file_index, .. } => {
                    jobs.files[*file_index].size
                }
                _ => 0,
            })
            .sum();
        let mut artifacts: Vec<_> = jobs.required_artifacts.iter().collect();
        artifacts.sort_by(|left, right| left.id().cmp(right.id()));
        for artifact in artifacts {
            request.control.checkpoint()?;
            visitor(InstallationPlanEvent::Artifact {
                id: artifact.id().to_owned(),
                available_locally: existing.contains(artifact),
            })?;
        }
        Ok(InstallationPlanSummary {
            snapshot_id: target.snapshot_id.clone(),
            content_inspected: verified,
            entry_count: target.entries.len(),
            required_artifact_count: jobs.required_artifact_count,
            retained_file_bytes,
            source_reuse_bytes,
            staging_file_bytes: execution_manifest.files().map(|file| file.size).sum(),
            staging_space_is_estimate: true,
        })
    })
}

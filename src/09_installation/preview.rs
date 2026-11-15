//! Streamed installation change previews. Execution always reinspects its inputs.
use crate::Result;
use crate::installation::contracts::InstallationRequest;
use crate::installation::contracts::SnapshotReconstructSource;
use crate::installation::execute::jobs::{ReconstructJob, ReconstructPipelinePlan};
use crate::installation::inspect::VerifyMode;
use crate::installation::install::InstallationPublication;
use crate::installation::plan::InstallationChanges;
use crate::installation::plan::build_reconstruct_pipeline_plan;
use crate::installation::plan::inspect_installation_changes;
use crate::installation::prepare::LoadedRepoSnapshot;
use crate::installation::prepare::load_repo_snapshot_manifest;
use crate::installation::source::resolve_verified_source;
use crate::installation::staging::validate_installation_paths;
use crate::manifest::snapshot::SnapshotManifest;
use crate::repo::artifacts::required_artifacts;
use crate::repo::coordination::read_with_control;
use crate::repo::index::load_chunk_index;
use crate::repo::snapshot_manifests::load_snapshot_manifest;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};

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
    request: &InstallationRequest,
    publication: InstallationPublication,
    inspection: InstallationInspection,
    visitor: &mut dyn FnMut(InstallationPlanEvent) -> Result<()>,
) -> Result<InstallationPlanSummary> {
    read_with_control(&request.repo_path, &request.control, || {
        request.control.checkpoint()?;
        validate_installation_paths(
            &request.destination_dir,
            request.staging_dir.as_deref(),
            &request.repo_path,
        )?;
        let LoadedRepoSnapshot {
            repo_paths,
            snapshot_manifest: target_manifest,
        } = load_repo_snapshot_manifest(&request.repo_path, &request.snapshot_id)?;
        let chunk_index = load_chunk_index(&repo_paths)?;
        let content_inspected = inspection == InstallationInspection::Content;
        let changes = inspect_installation_changes(
            &target_manifest,
            Some(&chunk_index),
            &request.destination_dir,
            if content_inspected {
                VerifyMode::Fast
            } else {
                VerifyMode::MetadataOnly
            },
            &request.runtime_hints,
            &request.control,
            None,
        )?;
        let selective = publication.is_selective();
        let entry_actions = EntryActions::new(&changes, selective, content_inspected);
        let retained_file_bytes =
            entry_actions.visit_target_entries(request, &target_manifest, visitor)?;
        visit_extra_files(request, publication, &changes, visitor)?;

        let execution_manifest = if selective {
            &changes.affected_manifest
        } else {
            &target_manifest
        };
        let source_manifest = request
            .source_snapshot_id
            .as_ref()
            .map(|id| load_snapshot_manifest(&repo_paths, id))
            .transpose()?;
        let source = if content_inspected {
            resolve_verified_source(
                source_manifest
                    .as_ref()
                    .map(|manifest| SnapshotReconstructSource {
                        snapshot_manifest: manifest,
                        destination_dir: request
                            .source_dir
                            .as_deref()
                            .unwrap_or(&request.destination_dir),
                    }),
                &request.runtime_hints,
                &request.control,
            )?
        } else {
            None
        };
        let existing_artifacts = required_artifacts(execution_manifest, &chunk_index)?
            .into_iter()
            .filter(|artifact| {
                artifact
                    .local_path(&repo_paths)
                    .is_ok_and(|path| path.is_file())
            })
            .collect();
        let plan = build_reconstruct_pipeline_plan(
            execution_manifest,
            &chunk_index,
            &repo_paths,
            &existing_artifacts,
            source.as_ref().map(|source| source.source()),
        )?;
        let mut artifacts: Vec<_> = plan.required_artifacts.iter().collect();
        artifacts.sort_by_cached_key(|artifact| artifact.id().into_owned());
        for artifact in artifacts {
            request.control.checkpoint()?;
            visitor(InstallationPlanEvent::Artifact {
                id: artifact.id().into_owned(),
                available_locally: existing_artifacts.contains(artifact),
            })?;
        }
        Ok(InstallationPlanSummary {
            snapshot_id: target_manifest.snapshot_id.clone(),
            content_inspected,
            entry_count: target_manifest.entries.len(),
            required_artifact_count: plan.required_artifact_count,
            retained_file_bytes,
            source_reuse_bytes: source_reuse_bytes(&plan),
            staging_file_bytes: execution_manifest.files().map(|file| file.size).sum(),
            staging_space_is_estimate: true,
        })
    })
}

/// Decides what installing does to each target entry.
struct EntryActions<'a> {
    missing: HashSet<&'a str>,
    corrupt: HashMap<&'a str, &'a str>,
    metadata_updates: HashSet<&'a str>,
    selective: bool,
    content_inspected: bool,
}

impl<'a> EntryActions<'a> {
    fn new(changes: &'a InstallationChanges, selective: bool, content_inspected: bool) -> Self {
        let classification = &changes.classification;
        Self {
            missing: classification
                .missing_files
                .iter()
                .map(|issue| issue.path.as_str())
                .collect(),
            corrupt: classification
                .corrupt_files
                .iter()
                .map(|issue| (issue.path.as_str(), issue.detail.as_str()))
                .collect(),
            metadata_updates: changes
                .metadata_updates
                .iter()
                .map(|entry| entry.path())
                .collect(),
            selective,
            content_inspected,
        }
    }

    fn action(&self, path: &str) -> (InstallationAction, &'a str) {
        if self.missing.contains(path) {
            (InstallationAction::Add, "missing from destination")
        } else if let Some(&reason) = self.corrupt.get(path) {
            (InstallationAction::Replace, reason)
        } else if !self.selective {
            (InstallationAction::Replace, "complete staged replacement")
        } else if self.metadata_updates.contains(path) {
            (
                InstallationAction::UpdateMetadata,
                "content matches; metadata differs",
            )
        } else if self.content_inspected {
            (InstallationAction::Keep, "matches target")
        } else {
            (
                InstallationAction::Keep,
                "metadata matches; content unverified",
            )
        }
    }

    /// Reports each target entry in path order and returns the bytes of the
    /// files kept in place.
    fn visit_target_entries(
        &self,
        request: &InstallationRequest,
        target_manifest: &SnapshotManifest,
        visitor: &mut dyn FnMut(InstallationPlanEvent) -> Result<()>,
    ) -> Result<u64> {
        let mut retained_file_bytes = 0;
        let entries: BTreeMap<_, _> = target_manifest
            .entries
            .iter()
            .map(|entry| (entry.path(), entry))
            .collect();
        for (path, entry) in entries {
            request.control.checkpoint()?;
            let (action, reason) = self.action(path);
            if matches!(
                action,
                InstallationAction::Keep | InstallationAction::UpdateMetadata
            ) {
                retained_file_bytes += entry.as_file().map_or_default(|file| file.size);
            }
            visitor(InstallationPlanEvent::Entry {
                path: path.to_owned(),
                action,
                reason: reason.to_owned(),
                content_verified: self.content_inspected
                    && !self.missing.contains(path)
                    && !self.corrupt.contains_key(path),
            })?;
        }
        Ok(retained_file_bytes)
    }
}

/// Reports destination files that the target snapshot does not contain.
fn visit_extra_files(
    request: &InstallationRequest,
    publication: InstallationPublication,
    changes: &InstallationChanges,
    visitor: &mut dyn FnMut(InstallationPlanEvent) -> Result<()>,
) -> Result<()> {
    let action = if publication.removes_extra_files() {
        InstallationAction::Remove
    } else {
        InstallationAction::PreserveExtra
    };
    let mut extras: Vec<_> = changes.classification.extra_files.iter().collect();
    extras.sort_by(|left, right| left.path.cmp(&right.path));
    for extra in extras {
        request.control.checkpoint()?;
        visitor(InstallationPlanEvent::Entry {
            path: extra.path.clone(),
            action,
            reason: "absent from target snapshot".into(),
            content_verified: false,
        })?;
    }
    Ok(())
}

/// Bytes the plan writes from the verified source instead of from artifacts.
fn source_reuse_bytes(plan: &ReconstructPipelinePlan) -> u64 {
    plan.jobs
        .iter()
        .map(|job| match job {
            ReconstructJob::SourceChunk(job) => {
                u64::from(job.raw_size) * job.targets().count() as u64
            }
            ReconstructJob::CopySourceFile { file_index, .. } => plan.files[*file_index].size,
            ReconstructJob::Artifact(_) => 0,
        })
        .sum()
}

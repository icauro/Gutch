//! Shared installation entry point, with full reconstruction as a preset.

use crate::Result;
use crate::installation::contracts::{
    InstallationReconstructRequest, InstallationReconstructResult, InstallationRepairOutcome,
};
use crate::installation::execute::apply_reconstruct_pipeline;
use crate::installation::execute::jobs::ReconstructPipelineApplyRequest;
use crate::installation::execute::publish::Publication;
use crate::installation::inspect::InstallationVerifyMode;
use crate::installation::plan::inspect_installation_changes;
use crate::installation::prepare::LoadedRepoSnapshot;
use crate::installation::prepare::LoadedSnapshotSource;
use crate::installation::prepare::load_repo_snapshot_manifest;
use crate::installation::prepare::{ReconstructInputs, prepare_reconstruction};
use crate::installation::staging::prepare_installation_destination;
use crate::repo::artifacts::required_artifacts;
use crate::repo::coordination::read_with_control;
use crate::repo::index::load_chunk_index;
use crate::transfer::types::TransferConcurrencyPolicy;

/// How an installation is written. The default changes only missing or
/// damaged entries and keeps extra files, as repair does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InstallationPublication {
    /// Rebuild in staging and replace the whole destination, as reconstruct does.
    ReplaceDirectory,
    /// Change only missing or damaged entries in place.
    Selective { extra_files: ExtraFilePolicy },
}

impl Default for InstallationPublication {
    fn default() -> Self {
        Self::Selective {
            extra_files: ExtraFilePolicy::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExtraFilePolicy {
    #[default]
    Preserve,
    Remove,
}
#[derive(Debug, Clone)]
pub(crate) struct InstallationResult {
    pub(crate) reconstruction: InstallationReconstructResult,
    pub(crate) repair: Option<InstallationRepairOutcome>,
}

/// Full reconstruction: build in staging, then replace the destination.
pub fn reconstruct_installation(
    request: &InstallationReconstructRequest,
) -> Result<InstallationReconstructResult> {
    install_snapshot_from_repo(request, InstallationPublication::ReplaceDirectory)
        .map(|result| result.reconstruction)
}

pub(crate) fn install_snapshot_from_repo(
    request: &InstallationReconstructRequest,
    publication: InstallationPublication,
) -> Result<InstallationResult> {
    request.control.checkpoint()?;
    read_with_control(&request.repo_root, &request.control.clone(), || {
        request.control.checkpoint()?;
        let _operation_span = request.control.profile_span(match publication {
            InstallationPublication::ReplaceDirectory => "installation.reconstruct",
            InstallationPublication::Selective { .. } => "installation.repair",
        });
        let LoadedRepoSnapshot {
            repo_paths,
            snapshot_manifest,
        } = load_repo_snapshot_manifest(&request.repo_root, &request.snapshot_id)?;
        let chunk_index = load_chunk_index(&repo_paths)?;
        let _promotion_lock = prepare_installation_destination(
            &snapshot_manifest,
            &request.destination_dir,
            request.staging_dir.as_deref(),
            &request.repo_root,
            &request.control,
        )?;
        let changes = if publication.is_selective() {
            Some(inspect_installation_changes(
                &snapshot_manifest,
                Some(&chunk_index),
                &request.destination_dir,
                InstallationVerifyMode::Fast,
                &request.runtime_hints,
                &request.control,
                None,
            )?)
        } else {
            None
        };
        let execution_manifest = changes
            .as_ref()
            .map(|changes| &changes.affected_manifest)
            .unwrap_or(&snapshot_manifest);
        let required_artifacts = required_artifacts(execution_manifest, &chunk_index)?;
        let existing_artifacts = required_artifacts
            .iter()
            .filter(|artifact| {
                artifact
                    .local_path(&repo_paths)
                    .is_ok_and(|path| path.is_file())
            })
            .cloned()
            .collect();
        let source = LoadedSnapshotSource::load(
            &repo_paths,
            request.reuse_snapshot_id.as_deref(),
            request.reuse_from.as_deref(),
            &request.destination_dir,
        )?;
        let prepared = prepare_reconstruction(ReconstructInputs {
            snapshot_manifest: execution_manifest,
            chunk_index: &chunk_index,
            repo_paths: &repo_paths,
            existing_artifacts: &existing_artifacts,
            source: source.as_ref().map(LoadedSnapshotSource::as_source),
            runtime_hints: &request.runtime_hints,
            control: &request.control,
        })?;
        let (mut reconstruction, repair) = apply_reconstruct_pipeline(
            ReconstructPipelineApplyRequest {
                publication: match changes.as_ref() {
                    Some(changes) => Publication::Selective {
                        target: &snapshot_manifest,
                        changes,
                        remove_extra_files: publication.removes_extra_files(),
                    },
                    None => Publication::ReplaceDirectory,
                },
                repo_root: &repo_paths.repo_root,
                snapshot_manifest: execution_manifest,
                destination_dir: &request.destination_dir,
                staging_dir: request.staging_dir.as_deref(),
                source_destination_dir: prepared.source_directory,
                runtime_hints: &request.runtime_hints,
                existing_artifacts: &existing_artifacts,
                pending_artifact_downloads: &[],
                transfer_policy: TransferConcurrencyPolicy::default(),
                control: &request.control,
                download_client: None,
                transfer_progress: None,
            },
            prepared.plan,
        )?;
        reconstruction.snapshot_manifest = snapshot_manifest;
        Ok(InstallationResult {
            reconstruction,
            repair,
        })
    })
}

impl InstallationPublication {
    pub(crate) fn is_selective(self) -> bool {
        matches!(self, Self::Selective { .. })
    }

    pub(crate) fn removes_extra_files(self) -> bool {
        !matches!(
            self,
            Self::Selective {
                extra_files: ExtraFilePolicy::Preserve
            }
        )
    }
}

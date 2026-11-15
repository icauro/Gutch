use super::apply::prepare_installation_destination;
use super::prepare::{ReconstructionInputs, prepare_reconstruction};
use super::types::{
    InstallationReconstructResult, InstallationRequest, SnapshotReconstructSource,
    SnapshotRepairResult,
};
use crate::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InstallationPublication {
    ReplaceDirectory,
    Selective { extra_files: ExtraFilePolicy },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExtraFilePolicy {
    Preserve,
    Remove,
}
#[derive(Debug, Clone)]
pub struct InstallationResult {
    pub reconstruction: InstallationReconstructResult,
    pub repair: Option<SnapshotRepairResult>,
}

pub fn install_snapshot_from_repo(
    request: InstallationRequest,
    publication: InstallationPublication,
) -> Result<InstallationResult> {
    request.control.checkpoint()?;
    let operation_root = request.repo_path.to_path_buf();
    crate::repo::coordination::read_with_control(&operation_root, &request.control.clone(), || {
        request.control.checkpoint()?;
        let _operation_span = request.control.profile_span(match publication {
            InstallationPublication::ReplaceDirectory => "installation.reconstruct",
            InstallationPublication::Selective { .. } => "installation.repair",
        });
        let super::prepare::LoadedRepoSnapshot {
            repo_paths,
            repo_manifest,
            snapshot_manifest,
        } = super::prepare::load_repo_snapshot_manifest(&request.repo_path, &request.snapshot_id)?;
        let chunk_index = crate::repo::index::load_chunk_index(&repo_paths)?;
        let _promotion_lock = prepare_installation_destination(
            &snapshot_manifest,
            &request.destination_dir,
            request.staging_dir.as_deref(),
            &request.repo_path,
            &request.control,
        )?;
        let changes = if publication.is_selective() {
            Some(super::plan::inspect_installation_changes(
                &snapshot_manifest,
                Some(&chunk_index),
                &request.destination_dir,
                super::inspect::VerifyMode::Fast,
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
        let required_artifacts =
            crate::repo::artifacts::required_artifacts(execution_manifest, &chunk_index)?;
        let existing_artifacts = required_artifacts
            .iter()
            .filter(|artifact| {
                artifact
                    .local_path(&repo_paths)
                    .is_ok_and(|path| path.is_file())
            })
            .cloned()
            .collect();
        let source_manifest = match request.source_snapshot_id.as_ref() {
            Some(source_snapshot_id) => {
                Some(crate::repo::snapshot_manifests::load_snapshot_manifest(
                    &repo_paths,
                    source_snapshot_id,
                )?)
            }
            None => None,
        };
        let source_destination_dir = source_manifest.as_ref().map(|_| {
            request
                .source_dir
                .clone()
                .unwrap_or_else(|| request.destination_dir.clone())
        });
        let source = source_manifest
            .as_ref()
            .zip(source_destination_dir.as_ref())
            .map(
                |(source_manifest, source_destination_dir)| SnapshotReconstructSource {
                    snapshot_manifest: source_manifest,
                    destination_dir: source_destination_dir,
                },
            );
        let prepared = prepare_reconstruction(ReconstructionInputs {
            snapshot_manifest: execution_manifest,
            chunk_index: &chunk_index,
            repo_paths: &repo_paths,
            existing_artifacts: &existing_artifacts,
            source,
            runtime_hints: &request.runtime_hints,
            control: &request.control,
        })?;
        let (mut reconstruction, repair) = super::execute::apply_reconstruct_pipeline(
            crate::installation::jobs::ReconstructPipelineApplyRequest {
                publication: match changes.as_ref() {
                    Some(changes) => super::publish::Publication::Selective {
                        target: &snapshot_manifest,
                        changes,
                        remove_extra_files: publication.removes_extra_files(),
                    },
                    None => super::publish::Publication::ReplaceDirectory,
                },
                repo_root: &repo_paths.repo_root,
                compression_profile: repo_manifest.compression_profile(),
                snapshot_manifest: execution_manifest,
                destination_dir: &request.destination_dir,
                staging_dir: request.staging_dir.as_deref(),
                source_destination_dir: prepared.source_directory,
                runtime_hints: &request.runtime_hints,
                existing_artifacts: &existing_artifacts,
                pending_artifact_downloads: &[],
                transfer_policy: crate::transfer::types::TransferConcurrencyPolicy::default(),
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

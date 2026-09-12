use super::apply::{VerifiedSource, prepare_directory_promotion, resolve_verified_source};
use super::types::{
    SnapshotReconstructFromRepoRequest, SnapshotReconstructFromRepoResult,
    SnapshotReconstructSource, SnapshotRepairResult,
};
use crate::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InstallationPublication {
    ReplaceDirectory,
    Selective,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExtraFilePolicy {
    Preserve,
    Remove,
}
#[derive(Debug, Clone)]
pub struct InstallationResult {
    pub reconstruction: SnapshotReconstructFromRepoResult,
    pub repair: Option<SnapshotRepairResult>,
}

pub fn install_snapshot_from_repo(
    request: SnapshotReconstructFromRepoRequest,
    publication: InstallationPublication,
    extra_files: ExtraFilePolicy,
) -> Result<InstallationResult> {
    if publication == InstallationPublication::ReplaceDirectory
        && extra_files == ExtraFilePolicy::Preserve
    {
        return Err(crate::Error::manifest(
            "complete replacement requires removing extra files; use selective publication to preserve them",
        ));
    }
    request.control.checkpoint()?;
    let operation_root = (&request.repo_path).to_path_buf();
    crate::repo::coordination::read_with_control(&operation_root, &request.control.clone(), || {
        request.control.checkpoint()?;
        let _operation_span = request.control.profile_span(match publication {
            InstallationPublication::ReplaceDirectory => "installation.reconstruct",
            InstallationPublication::Selective => "installation.repair",
        });
        let (repo_paths, snapshot_manifest) = super::snapshot_repo::load_repo_snapshot_manifest(
            &request.repo_path,
            &request.snapshot_id,
        )?;
        let (_, repo_manifest) =
            crate::repo::paths::load_gutch_repo_manifest(&repo_paths.repo_root)?;
        let chunk_index = crate::repo::index::load_chunk_index(&repo_paths)?;
        crate::filesystem::paths::validate_destination_paths(
            &request.destination_dir,
            snapshot_manifest.entries.iter().map(|entry| entry.path()),
        )?;
        let _promotion_lock = prepare_directory_promotion(
            &request.destination_dir,
            request.staging_dir.as_deref(),
            &request.repo_path,
            &request.control,
        )?;
        let changes = if publication == InstallationPublication::Selective {
            Some(super::plan::inspect_installation_changes(
                &snapshot_manifest,
                Some(&chunk_index),
                &request.destination_dir,
                extra_files == ExtraFilePolicy::Remove,
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
        let verified_source =
            resolve_verified_source(source, &request.runtime_hints, &request.control)?;
        let pipeline_plan = {
            let _plan_span = request
                .control
                .profile_span("installation.reconstruct.prepare.plan");
            crate::installation::plan::build_reconstruct_pipeline_plan(
                execution_manifest,
                &chunk_index,
                &repo_paths,
                &existing_artifacts,
                verified_source.as_ref().map(VerifiedSource::source),
            )?
        };
        let (mut reconstruction, repair) = super::execute::apply_reconstruct_pipeline(
            crate::installation::jobs::ReconstructPipelineApplyRequest {
                publication: match changes.as_ref() {
                    Some(changes) => super::publish::Publication::Selective {
                        target: &snapshot_manifest,
                        changes,
                        remove_extra_files: extra_files == ExtraFilePolicy::Remove,
                    },
                    None => super::publish::Publication::ReplaceDirectory,
                },
                repo_root: &repo_paths.repo_root,
                compression_profile: repo_manifest.compression_profile(),
                snapshot_manifest: execution_manifest,
                destination_dir: &request.destination_dir,
                staging_dir: request.staging_dir.as_deref(),
                source_destination_dir,
                runtime_hints: &request.runtime_hints,
                existing_artifacts: &existing_artifacts,
                pending_bundle_downloads: &[],
                transfer_policy: crate::transfer::types::TransferConcurrencyPolicy::default(),
                control: &request.control,
                download_client: None,
                transfer_progress: None,
            },
            pipeline_plan,
        )?;
        reconstruction.snapshot_manifest = snapshot_manifest;
        Ok(InstallationResult {
            reconstruction,
            repair,
        })
    })
}

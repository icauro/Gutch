//! Repair: replaces only missing or damaged entries in a destination.

use crate::Result;

use crate::installation::contracts::InstallationRepairPlanRequest;
use crate::installation::contracts::InstallationRepairPlanResult;
use crate::installation::contracts::InstallationRepairRequest;
use crate::installation::contracts::InstallationRepairResult;

use crate::installation::contracts::InstallationRequest;
use crate::installation::inspect::VerifyMode;
use crate::installation::install::ExtraFilePolicy;
use crate::installation::install::InstallationPublication;
use crate::installation::install::install_snapshot_from_repo;
use crate::installation::plan::inspect_installation_changes;
use crate::installation::prepare::LoadedRepoSnapshot;
use crate::installation::prepare::load_repo_snapshot_manifest;
use crate::repo::coordination::read_with_control;
use crate::repo::index::load_chunk_index;
pub fn repair_installation(request: InstallationRepairRequest) -> Result<InstallationRepairResult> {
    let result = install_snapshot_from_repo(
        InstallationRequest {
            repo_path: request.repo_path,
            snapshot_id: request.snapshot_id,
            destination_dir: request.destination_dir,
            staging_dir: request.staging_dir,
            source_snapshot_id: None,
            source_dir: None,
            runtime_hints: request.runtime_hints,
            control: request.control,
        },
        InstallationPublication::Selective {
            extra_files: if request.remove_extra_files {
                ExtraFilePolicy::Remove
            } else {
                ExtraFilePolicy::Preserve
            },
        },
    )?;
    Ok(InstallationRepairResult {
        repo_root: result.reconstruction.repo_root,
        snapshot_manifest: result.reconstruction.snapshot_manifest,
        resolved_bundle_file_count: result.reconstruction.resolved_bundle_file_count,
        repair_result: result
            .repair
            .expect("selective publication returns repair result"),
    })
}

pub fn plan_installation_repair(
    request: InstallationRepairPlanRequest,
) -> Result<InstallationRepairPlanResult> {
    request.control.checkpoint()?;
    let operation_root = request.repo_path.to_path_buf();
    read_with_control(&operation_root, &request.control.clone(), || {
        request.control.checkpoint()?;
        let LoadedRepoSnapshot {
            repo_paths,
            snapshot_manifest,
        } = load_repo_snapshot_manifest(&request.repo_path, &request.snapshot_id)?;
        let chunk_index = load_chunk_index(&repo_paths)?;
        let repair_plan = inspect_installation_changes(
            &snapshot_manifest,
            Some(&chunk_index),
            &request.destination_dir,
            VerifyMode::Fast,
            &request.runtime_hints,
            &request.control,
            None,
        )?;

        Ok(InstallationRepairPlanResult {
            repo_root: repo_paths.repo_root,
            snapshot_manifest,
            missing_file_count: repair_plan.missing_file_count,
            corrupt_file_count: repair_plan.corrupt_file_count,
            extra_file_count: repair_plan.extra_file_count,
            needs_rebuild: repair_plan.needs_rebuild,
            required_artifact_ids: repair_plan
                .required_artifacts
                .into_iter()
                .map(|artifact| artifact.id().to_string())
                .collect(),
        })
    })
}

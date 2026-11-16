//! Repair: replaces only missing or damaged entries in a destination.

use crate::Result;

use crate::installation::contracts::InstallationRepairPlan;
use crate::installation::contracts::InstallationRepairRequest;
use crate::installation::contracts::InstallationRepairResult;

use crate::installation::contracts::InstallationReconstructRequest;
use crate::installation::inspect::InstallationVerifyMode;
use crate::installation::install::ExtraFilePolicy;
use crate::installation::install::InstallationPublication;
use crate::installation::install::install_snapshot_from_repo;
use crate::installation::plan::inspect_installation_changes;
use crate::installation::prepare::LoadedRepoSnapshot;
use crate::installation::prepare::load_repo_snapshot_manifest;
use crate::repo::coordination::read_with_control;
use crate::repo::index::load_chunk_index;

pub fn repair_installation(
    request: &InstallationRepairRequest,
) -> Result<InstallationRepairResult> {
    let result = install_snapshot_from_repo(
        &InstallationReconstructRequest {
            repo_root: request.repo_root.clone(),
            snapshot_id: request.snapshot_id.clone(),
            destination_dir: request.destination_dir.clone(),
            staging_dir: request.staging_dir.clone(),
            reuse_snapshot_id: None,
            reuse_dir: None,
            runtime_hints: request.runtime_hints,
            control: request.control.clone(),
        },
        InstallationPublication::Selective {
            extra_files: if request.remove_extra_files {
                ExtraFilePolicy::Remove
            } else {
                ExtraFilePolicy::Preserve
            },
        },
    )?;
    let reconstruction = result.reconstruction;
    let repair = result
        .repair
        .expect("selective publication returns repair result");
    Ok(InstallationRepairResult {
        repo_root: reconstruction.repo_root,
        snapshot_manifest: reconstruction.snapshot_manifest,
        snapshot_id: reconstruction.snapshot_id,
        destination_dir: reconstruction.destination_dir,
        missing_file_count: repair.missing_file_count,
        corrupt_file_count: repair.corrupt_file_count,
        extra_file_count: repair.extra_file_count,
        removed_extra_file_count: repair.removed_extra_file_count,
        required_bundle_count: reconstruction.required_bundle_count,
    })
}

/// Classifies the destination and lists the artifacts a repair would need, without changing it.
pub fn plan_repair_installation(
    request: &InstallationRepairRequest,
) -> Result<InstallationRepairPlan> {
    request.control.checkpoint()?;
    read_with_control(&request.repo_root, &request.control.clone(), || {
        request.control.checkpoint()?;
        let LoadedRepoSnapshot {
            repo_paths,
            snapshot_manifest,
        } = load_repo_snapshot_manifest(&request.repo_root, &request.snapshot_id)?;
        let chunk_index = load_chunk_index(&repo_paths)?;
        let repair_plan = inspect_installation_changes(
            &snapshot_manifest,
            Some(&chunk_index),
            &request.destination_dir,
            InstallationVerifyMode::Fast,
            &request.runtime_hints,
            &request.control,
            None,
        )?;

        Ok(InstallationRepairPlan {
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

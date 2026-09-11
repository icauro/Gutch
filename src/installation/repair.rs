use crate::Result;

use super::types::SnapshotRepairFromRepoRequest;
use super::types::SnapshotRepairFromRepoResult;
use super::types::SnapshotRepairPlanFromRepoRequest;
use super::types::SnapshotRepairPlanFromRepoResult;

use super::plan::inspect_installation_changes;
pub fn repair_snapshot_from_repo(
    request: SnapshotRepairFromRepoRequest,
) -> Result<SnapshotRepairFromRepoResult> {
    let result = super::operation::install_snapshot_from_repo(
        super::types::SnapshotReconstructFromRepoRequest {
            repo_path: request.repo_path,
            snapshot_id: request.snapshot_id,
            destination_dir: request.destination_dir,
            staging_dir: request.staging_dir,
            source_snapshot_id: None,
            source_dir: None,
            runtime_hints: request.runtime_hints,
            control: request.control,
        },
        super::operation::InstallationPublication::Selective,
        if request.remove_added_files {
            super::operation::ExtraFilePolicy::Remove
        } else {
            super::operation::ExtraFilePolicy::Preserve
        },
    )?;
    Ok(SnapshotRepairFromRepoResult {
        repo_root: result.reconstruction.repo_root,
        snapshot_manifest: result.reconstruction.snapshot_manifest,
        resolved_bundle_file_count: result.reconstruction.resolved_bundle_file_count,
        repair_result: result
            .repair
            .expect("selective publication returns repair result"),
    })
}

pub fn plan_repair_snapshot_from_repo(
    request: SnapshotRepairPlanFromRepoRequest,
) -> Result<SnapshotRepairPlanFromRepoResult> {
    request.control.checkpoint()?;
    let operation_root = (&request.repo_path).to_path_buf();
    crate::repo::coordination::read_with_control(&operation_root, &request.control.clone(), || {
        request.control.checkpoint()?;
        let (repo_paths, snapshot_manifest) = super::snapshot_repo::load_repo_snapshot_manifest(
            &request.repo_path,
            &request.snapshot_id,
        )?;
        let chunk_index = crate::repo::index::load_chunk_index(&repo_paths)?;
        let repair_plan = inspect_installation_changes(
            &snapshot_manifest,
            Some(&chunk_index),
            &request.destination_dir,
            request.remove_added_files,
            &request.runtime_hints,
            &request.control,
            None,
        )?;

        Ok(SnapshotRepairPlanFromRepoResult {
            repo_root: repo_paths.repo_root,
            snapshot_manifest,
            missing_file_count: repair_plan.missing_file_count,
            corrupt_file_count: repair_plan.corrupt_file_count,
            extra_file_count: repair_plan.extra_file_count,
            needs_rebuild: repair_plan.needs_rebuild,
            required_bundle_ids: repair_plan
                .required_artifacts
                .into_iter()
                .map(|artifact| artifact.id().to_string())
                .collect(),
        })
    })
}

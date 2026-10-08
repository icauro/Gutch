use super::apply::build_snapshot_reconstruct_result;
use super::entries::{finalize_snapshot_entries, prepare_snapshot_entries};
use super::jobs::{ReconstructPipelineApplyRequest, ReconstructPipelinePlan};
use super::pipeline::{ExecuteReconstructPipelineRequest, run_reconstruction_jobs};
use super::types::InstallationReconstructResult;
use crate::profile::ProfileInstant;
use crate::runtime::plan_runtime;
use crate::{Error, Result};
use std::path::Path;
pub(crate) fn apply_reconstruct_pipeline(
    request: ReconstructPipelineApplyRequest<'_>,
    pipeline_plan: ReconstructPipelinePlan,
) -> Result<(
    InstallationReconstructResult,
    Option<super::types::SnapshotRepairResult>,
)> {
    let _output_stage = request.control.output_stage();
    request.control.checkpoint()?;
    let operation_started_at = ProfileInstant::start(request.control.profiler());
    let _operation_span = request
        .control
        .profile_span("installation.reconstruct.apply");
    let prepare_started_at = ProfileInstant::start(request.control.profiler());
    let reconstruct_worker_count = plan_runtime(request.runtime_hints).worker_count;
    let reconstruct_pool = if request.snapshot_manifest.entries.is_empty() {
        None
    } else {
        let _pool_span = request
            .control
            .profile_span("installation.reconstruct.prepare.pool");
        Some(
            crate::runtime::build_thread_pool(reconstruct_worker_count, "Reconstruction worker")
                .map_err(|error| Error::reconstruct(error.to_string()))?,
        )
    };
    let staging = {
        let _staging_span = request
            .control
            .profile_span("installation.reconstruct.prepare.staging");
        let staging_dir = request
            .staging_dir
            .map(Path::to_path_buf)
            .unwrap_or_else(|| {
                super::apply::default_snapshot_staging_dir(
                    request.destination_dir,
                    &request.snapshot_manifest.snapshot_id,
                )
            });
        crate::filesystem::paths::validate_destination_paths(
            request.destination_dir,
            request
                .snapshot_manifest
                .entries
                .iter()
                .map(|entry| entry.path()),
        )?;
        // tempfile appends a six-character random suffix to the operation prefix.
        crate::filesystem::paths::validate_destination_paths(
            &staging_dir.join(".gutch-operation-XXXXXX"),
            request
                .snapshot_manifest
                .entries
                .iter()
                .map(|entry| entry.path()),
        )?;
        super::apply::create_snapshot_staging(
            request.destination_dir,
            &staging_dir,
            request.repo_root,
            request.source_destination_dir.as_deref(),
        )?
    };
    let staging_dir = staging.path().to_path_buf();

    (|| -> Result<(InstallationReconstructResult, Option<super::types::SnapshotRepairResult>)> {
        let output_paths = {
            let _prepare_span = request
                .control
                .profile_span("installation.reconstruct.prepare.entries");
            prepare_snapshot_entries(
                request.snapshot_manifest,
                &staging_dir,
                request.control,
                reconstruct_pool.as_ref(),
            )?
        };
        let apply_prepare_wall = prepare_started_at.elapsed();
        let required_chunk_count = pipeline_plan.required_chunk_count;
        let required_bundle_count = pipeline_plan.required_bundle_count;
        let required_artifact_count = pipeline_plan.required_artifact_count;
        let reused_source_chunk_count = pipeline_plan.reused_source_chunk_count;
        let (mut reconstruct_timings, verify_timings) =
            run_reconstruction_jobs(ExecuteReconstructPipelineRequest {
                pipeline_plan,
                output_paths,
                snapshot_id: &request.snapshot_manifest.snapshot_id,
                compression_profile: &request.compression_profile,
                runtime_hints: request.runtime_hints,
                existing_artifacts: request.existing_artifacts,
                pending_artifact_downloads: request.pending_artifact_downloads,
                transfer_policy: request.transfer_policy,
                control: request.control,
                download_client: request.download_client,
                transfer_progress: request.transfer_progress,
                reconstruct_pool: reconstruct_pool.as_ref(),
            })?;
        reconstruct_timings.prepare_wall += apply_prepare_wall;
        reconstruct_timings.total = operation_started_at.elapsed();
        let repair_result = match &request.publication {
            super::publish::Publication::ReplaceDirectory => {
                let started = ProfileInstant::start(request.control.profiler());
                {
                    let _span = request.control.profile_span("installation.reconstruct.finalize.entries");
                    finalize_snapshot_entries(request.snapshot_manifest, &staging_dir, request.control,
                        reconstruct_worker_count, reconstruct_pool.as_ref())?;
                }
                reconstruct_timings.finalize_wall = started.elapsed();
                let started = ProfileInstant::start(request.control.profiler());
                {
                    let _span = request.control.profile_span("installation.reconstruct.finalize.promote");
                    request.control.checkpoint()?;
                    crate::filesystem::directory::replace_directory(&staging_dir, request.destination_dir)?;
                }
                reconstruct_timings.promote_wall = started.elapsed();
                None
            }
            super::publish::Publication::Selective { target, changes, remove_extra_files } => {
                let started = ProfileInstant::start(request.control.profiler());
                let result = super::publish::finalize_repair_result_after_reconstruction(
                    super::publish::FinalizeRepairRequest {
                        snapshot_manifest: target,
                        destination_dir: request.destination_dir,
                        staging_dir: &staging_dir,
                        remove_added_files: *remove_extra_files,
                        runtime_hints: request.runtime_hints,
                        repair_plan: changes,
                        control: request.control,
                        operation_pool: reconstruct_pool.as_ref(),
                    },
                )?;
                reconstruct_timings.promote_wall = started.elapsed();
                Some(result)
            }
        };
        reconstruct_timings.total = operation_started_at.elapsed();
        Ok((InstallationReconstructResult {
            repo_root: request.repo_root.to_path_buf(),
            snapshot_manifest: request.snapshot_manifest.clone(),
            reconstruct_result: build_snapshot_reconstruct_result(
                super::apply::BuildSnapshotReconstructResultRequest {
                    snapshot_manifest: request.snapshot_manifest,
                    destination_dir: request.destination_dir,
                    staging_dir: &staging_dir,
                    reused_source_chunk_count,
                    required_chunk_count,
                    required_bundle_count,
                    required_artifact_count,
                    reconstruct_timings,
                    verify_timings,
                },
            ),
            resolved_bundle_file_count: required_bundle_count,
            resolved_artifact_file_count: required_artifact_count,
            source_destination_dir: request.source_destination_dir.clone(),
        }, repair_result))
    })()
}

#[cfg(test)]
#[path = "execute_tests.rs"]
mod tests;

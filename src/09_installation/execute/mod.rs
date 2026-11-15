//! Runs a reconstruction plan: stages the tree, schedules artifact and source
//! jobs, verifies written files and publishes the result.

pub(crate) mod entries;
mod handles;
pub(crate) mod job_io;
pub(crate) mod jobs;
mod pipeline;
pub(crate) mod publish;
mod workers;

use crate::filesystem::directory::replace_directory;
use crate::filesystem::paths::validate_destination_paths;
use crate::installation::contracts::ReconstructTimings;
use crate::installation::contracts::SnapshotRepairResult;
use crate::installation::contracts::{InstallationReconstructResult, SnapshotReconstructResult};
use crate::installation::inspect::VerifyTimings;
use crate::installation::staging::create_snapshot_staging;
use crate::installation::staging::default_snapshot_staging_dir;
use crate::manifest::snapshot::SnapshotManifest;
use crate::profiling::ProfileInstant;
use crate::runtime::build_thread_pool;
use crate::runtime::plan_runtime;
use crate::{Error, Result};
use entries::{finalize_snapshot_entries, prepare_snapshot_entries};
use jobs::{ReconstructPipelineApplyRequest, ReconstructPipelinePlan};
use pipeline::{ExecuteReconstructPipelineRequest, run_reconstruction_jobs};
use std::path::Path;
pub(crate) fn apply_reconstruct_pipeline(
    request: ReconstructPipelineApplyRequest<'_>,
    pipeline_plan: ReconstructPipelinePlan,
) -> Result<(InstallationReconstructResult, Option<SnapshotRepairResult>)> {
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
            build_thread_pool(reconstruct_worker_count, "Reconstruction worker")
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
                default_snapshot_staging_dir(
                    request.destination_dir,
                    &request.snapshot_manifest.snapshot_id,
                )
            });
        validate_destination_paths(
            request.destination_dir,
            request
                .snapshot_manifest
                .entries
                .iter()
                .map(|entry| entry.path()),
        )?;
        // tempfile appends a six-character random suffix to the operation prefix.
        validate_destination_paths(
            &staging_dir.join(".gutch-operation-XXXXXX"),
            request
                .snapshot_manifest
                .entries
                .iter()
                .map(|entry| entry.path()),
        )?;
        create_snapshot_staging(
            request.destination_dir,
            &staging_dir,
            request.repo_root,
            request.source_destination_dir.as_deref(),
        )?
    };
    let staging_dir = staging.path().to_path_buf();

    (|| -> Result<(InstallationReconstructResult, Option<SnapshotRepairResult>)> {
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
            self::publish::Publication::ReplaceDirectory => {
                let started = ProfileInstant::start(request.control.profiler());
                {
                    let _span = request
                        .control
                        .profile_span("installation.reconstruct.finalize.entries");
                    finalize_snapshot_entries(
                        request.snapshot_manifest,
                        &staging_dir,
                        request.control,
                        reconstruct_worker_count,
                        reconstruct_pool.as_ref(),
                    )?;
                }
                reconstruct_timings.finalize_wall = started.elapsed();
                let started = ProfileInstant::start(request.control.profiler());
                {
                    let _span = request
                        .control
                        .profile_span("installation.reconstruct.finalize.promote");
                    request.control.checkpoint()?;
                    replace_directory(&staging_dir, request.destination_dir)?;
                }
                reconstruct_timings.promote_wall = started.elapsed();
                None
            }
            self::publish::Publication::Selective {
                target,
                changes,
                remove_extra_files,
            } => {
                let started = ProfileInstant::start(request.control.profiler());
                let result = self::publish::finalize_repair_result_after_reconstruction(
                    self::publish::FinalizeRepairRequest {
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
        Ok((
            InstallationReconstructResult {
                repo_root: request.repo_root.to_path_buf(),
                snapshot_manifest: request.snapshot_manifest.clone(),
                reconstruct_result: build_snapshot_reconstruct_result(
                    BuildSnapshotReconstructResultRequest {
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
            },
            repair_result,
        ))
    })()
}

pub(crate) struct BuildSnapshotReconstructResultRequest<'a> {
    pub snapshot_manifest: &'a SnapshotManifest,
    pub destination_dir: &'a Path,
    pub staging_dir: &'a Path,
    pub reused_source_chunk_count: usize,
    pub required_chunk_count: usize,
    pub required_bundle_count: usize,
    pub required_artifact_count: usize,
    pub reconstruct_timings: ReconstructTimings,
    pub verify_timings: VerifyTimings,
}

pub(crate) fn build_snapshot_reconstruct_result(
    request: BuildSnapshotReconstructResultRequest<'_>,
) -> SnapshotReconstructResult {
    SnapshotReconstructResult {
        snapshot_id: request.snapshot_manifest.snapshot_id.clone(),
        destination_dir: request.destination_dir.to_path_buf(),
        staging_dir: request.staging_dir.to_path_buf(),
        reused_source_chunk_count: request.reused_source_chunk_count,
        required_chunk_count: request.required_chunk_count,
        required_bundle_count: request.required_bundle_count,
        required_artifact_count: request.required_artifact_count,
        reconstruct_timings: request.reconstruct_timings,
        verify_timings: request.verify_timings,
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;

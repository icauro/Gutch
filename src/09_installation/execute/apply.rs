//! Applies a reconstruction plan: stages the tree, runs the jobs and publishes the result.

use super::entries::{finalize_snapshot_entries, prepare_snapshot_entries};
use super::jobs::{ReconstructPipelineApplyRequest, ReconstructPipelinePlan};
use super::pipeline::{ExecuteReconstructPipelineRequest, run_reconstruction_jobs};
use super::publish::{
    FinalizeRepairRequest, Publication, finalize_repair_result_after_reconstruction,
};
use crate::Result;
use crate::filesystem::directory::replace_directory;
use crate::filesystem::paths::validate_destination_paths;
use crate::installation::contracts::ReconstructTimings;
use crate::installation::contracts::SnapshotRepairResult;
use crate::installation::contracts::{InstallationReconstructResult, SnapshotReconstructResult};
use crate::installation::inspect::VerifyTimings;
use crate::installation::staging::SnapshotStaging;
use crate::installation::staging::create_snapshot_staging;
use crate::installation::staging::default_snapshot_staging_dir;
use crate::manifest::snapshot::SnapshotManifest;
use crate::profiling::ProfileInstant;
use crate::runtime::build_thread_pool;
use crate::runtime::plan_runtime;
use rayon::ThreadPool;
use std::path::Path;

pub(crate) fn apply_reconstruct_pipeline(
    request: ReconstructPipelineApplyRequest<'_>,
    pipeline_plan: ReconstructPipelinePlan,
) -> Result<(InstallationReconstructResult, Option<SnapshotRepairResult>)> {
    let control = request.control;
    let _output_stage = control.output_stage();
    control.checkpoint()?;
    let operation_started_at = ProfileInstant::start(control.profiler());
    let _operation_span = control.profile_span("installation.reconstruct.apply");
    let prepare_started_at = ProfileInstant::start(control.profiler());
    let worker_count = plan_runtime(request.runtime_hints).worker_count;
    let reconstruct_pool = if request.snapshot_manifest.entries.is_empty() {
        None
    } else {
        let _pool_span = control.profile_span("installation.reconstruct.prepare.pool");
        Some(build_thread_pool(worker_count, "Reconstruction worker")?)
    };
    let staging = create_staging(&request)?;
    let staging_dir = staging.path().to_path_buf();
    let output_paths = {
        let _prepare_span = control.profile_span("installation.reconstruct.prepare.entries");
        let complete_manifest = match &request.publication {
            Publication::Selective { target, .. } => target,
            Publication::ReplaceDirectory => request.snapshot_manifest,
        };
        prepare_snapshot_entries(
            request.snapshot_manifest,
            complete_manifest,
            &staging_dir,
            control,
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
            runtime_hints: request.runtime_hints,
            existing_artifacts: request.existing_artifacts,
            pending_artifact_downloads: request.pending_artifact_downloads,
            transfer_policy: request.transfer_policy,
            control,
            download_client: request.download_client,
            transfer_progress: request.transfer_progress,
            reconstruct_pool: reconstruct_pool.as_ref(),
        })?;
    reconstruct_timings.prepare_wall += apply_prepare_wall;
    reconstruct_timings.total = operation_started_at.elapsed();

    let repair_result = match &request.publication {
        Publication::ReplaceDirectory => {
            replace_with_staging(
                &request,
                &staging_dir,
                worker_count,
                reconstruct_pool.as_ref(),
                &mut reconstruct_timings,
            )?;
            None
        }
        Publication::Selective {
            target,
            changes,
            remove_extra_files,
        } => {
            let started = ProfileInstant::start(control.profiler());
            let result = finalize_repair_result_after_reconstruction(FinalizeRepairRequest {
                snapshot_manifest: target,
                destination_dir: request.destination_dir,
                staging_dir: &staging_dir,
                remove_extra_files: *remove_extra_files,
                runtime_hints: request.runtime_hints,
                repair_plan: changes,
                control,
                operation_pool: reconstruct_pool.as_ref(),
            })?;
            reconstruct_timings.promote_wall = started.elapsed();
            Some(result)
        }
    };
    reconstruct_timings.total = operation_started_at.elapsed();
    let reconstruct_result =
        build_snapshot_reconstruct_result(BuildSnapshotReconstructResultRequest {
            snapshot_manifest: request.snapshot_manifest,
            destination_dir: request.destination_dir,
            staging_dir: &staging_dir,
            reused_source_chunk_count,
            required_chunk_count,
            required_bundle_count,
            required_artifact_count,
            reconstruct_timings,
            verify_timings,
        });
    let installation_result = InstallationReconstructResult {
        repo_root: request.repo_root.to_path_buf(),
        snapshot_manifest: request.snapshot_manifest.clone(),
        reconstruct_result,
        resolved_bundle_file_count: required_bundle_count,
        resolved_artifact_file_count: required_artifact_count,
        source_destination_dir: request.source_destination_dir.clone(),
    };
    Ok((installation_result, repair_result))
}

/// Creates the staging directory after checking that every entry path fits
/// under both the destination and the staging location.
fn create_staging(request: &ReconstructPipelineApplyRequest<'_>) -> Result<SnapshotStaging> {
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
    let entry_paths = || {
        request
            .snapshot_manifest
            .entries
            .iter()
            .map(|entry| entry.path())
    };
    validate_destination_paths(request.destination_dir, entry_paths())?;
    // tempfile appends a six-character random suffix to the operation prefix.
    validate_destination_paths(&staging_dir.join(".gutch-operation-XXXXXX"), entry_paths())?;
    create_snapshot_staging(
        request.destination_dir,
        &staging_dir,
        request.repo_root,
        request.source_destination_dir.as_deref(),
    )
}

/// Applies entry metadata in the staging directory, then swaps it in for the
/// destination.
fn replace_with_staging(
    request: &ReconstructPipelineApplyRequest<'_>,
    staging_dir: &Path,
    worker_count: usize,
    reconstruct_pool: Option<&ThreadPool>,
    timings: &mut ReconstructTimings,
) -> Result<()> {
    let control = request.control;
    let started = ProfileInstant::start(control.profiler());
    {
        let _span = control.profile_span("installation.reconstruct.finalize.entries");
        finalize_snapshot_entries(
            request.snapshot_manifest,
            request.snapshot_manifest,
            staging_dir,
            control,
            worker_count,
            reconstruct_pool,
        )?;
    }
    timings.finalize_wall = started.elapsed();
    let started = ProfileInstant::start(control.profiler());
    {
        let _span = control.profile_span("installation.reconstruct.finalize.promote");
        control.checkpoint()?;
        replace_directory(staging_dir, request.destination_dir, control)?;
    }
    timings.promote_wall = started.elapsed();
    Ok(())
}

struct BuildSnapshotReconstructResultRequest<'a> {
    snapshot_manifest: &'a SnapshotManifest,
    destination_dir: &'a Path,
    staging_dir: &'a Path,
    reused_source_chunk_count: usize,
    required_chunk_count: usize,
    required_bundle_count: usize,
    required_artifact_count: usize,
    reconstruct_timings: ReconstructTimings,
    verify_timings: VerifyTimings,
}

fn build_snapshot_reconstruct_result(
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
#[path = "apply_tests.rs"]
mod tests;

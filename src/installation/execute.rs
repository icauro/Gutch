use crate::profile::ProfileInstant;
use std::collections::HashMap;
use std::collections::HashSet;
use std::fs::OpenOptions;
use std::panic::AssertUnwindSafe;
use std::panic::catch_unwind;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::mpsc;
use std::time::Duration;

use rayon::ThreadPool;

use super::apply::build_snapshot_reconstruct_result;
use super::types::InstallationReconstructResult;
use super::types::ReconstructTimings;
use crate::Error;
use crate::Result;
use crate::progress::GutchOperation;
use crate::progress::GutchOperationControl;
use crate::progress::GutchProgressEvent;
use crate::progress::GutchProgressPhase;
use crate::progress::emit_progress;
use crate::runtime::RuntimeResourceHints;
use crate::runtime::plan_runtime;
use crate::transfer::download::ArtifactDownloadEvent;
use crate::transfer::download::ArtifactDownloadScheduler;
use crate::transfer::download::PendingArtifactDownload;
use crate::transfer::progress::FetchTransferProgress;

use super::entries::*;
use super::handles::{OutputHandleCache, SourceHandleCache};
use super::io::{ReconstructedFileVerification, reconstruct_job, verify_reconstructed_output_file};
use super::jobs::*;
use super::plan::*;
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

pub(crate) struct ExecuteReconstructPipelineRequest<'a> {
    pub(crate) pipeline_plan: ReconstructPipelinePlan,
    pub(crate) output_paths: Arc<Vec<PathBuf>>,
    pub(crate) snapshot_id: &'a str,
    pub(crate) compression_profile: &'a crate::manifest::CompressionProfile,
    pub(crate) runtime_hints: &'a RuntimeResourceHints,
    pub(crate) existing_artifacts: &'a HashSet<crate::repo::artifacts::ArtifactRef>,
    pub(crate) pending_artifact_downloads: &'a [PendingArtifactDownload],
    pub(crate) transfer_policy: crate::transfer::types::TransferConcurrencyPolicy,
    pub(crate) control: &'a GutchOperationControl,
    pub(crate) download_client: Option<&'a crate::transfer::http::DownloadClient>,
    pub(crate) transfer_progress: Option<&'a Arc<FetchTransferProgress>>,
    pub(crate) reconstruct_pool: Option<&'a ThreadPool>,
}

enum ReconstructWorkItem {
    Job(usize),
    VerifyFile(usize),
}

/// Tracks only runnable and dispatched work. Jobs waiting for downloads are
/// owned by the artifact scheduler and enter this queue when ready.
#[derive(Default)]
struct ReconstructionWorkQueue {
    ready: std::collections::VecDeque<(ReconstructWorkItem, ProfileInstant)>,
    // Decremented when the coordinator consumes one terminal event, including failure.
    in_flight: usize,
}

struct AdmittedReconstructionWork {
    work: ReconstructWorkItem,
    permit: crate::runtime::MemoryPermit,
    queued_at: ProfileInstant,
}

impl ReconstructionWorkQueue {
    fn admit_next(
        &mut self,
        jobs: &[ReconstructJob],
        memory: &Arc<crate::runtime::MemoryBudget>,
    ) -> Result<Option<AdmittedReconstructionWork>> {
        for (index, (work, _)) in self.ready.iter().enumerate() {
            let bytes = match work {
                ReconstructWorkItem::Job(job_index) => jobs[*job_index].memory_reservation_bytes(),
                ReconstructWorkItem::VerifyFile(_) => 0,
            };
            if let Some(permit) = memory.try_acquire(bytes)? {
                let (work, queued_at) = self.ready.remove(index).expect("admitted queued work");
                self.in_flight += 1;
                return Ok(Some(AdmittedReconstructionWork {
                    work,
                    permit,
                    queued_at,
                }));
            }
        }
        Ok(None)
    }

    fn complete(&mut self) {
        self.in_flight -= 1;
    }
}

enum ReconstructJobEvent {
    Completed {
        job_index: usize,
        timings: Box<ReconstructTimings>,
    },
    Verified(ReconstructedFileVerification),
    Failed(Error),
}

struct ReconstructWorkContext {
    memory: Arc<crate::runtime::MemoryBudget>,
    source_handles: SourceHandleCache,
    jobs: Arc<Vec<ReconstructJob>>,
    file_plans: Arc<Vec<ReconstructPipelineFile>>,
    output_paths: Arc<Vec<PathBuf>>,
    precomputed_file_hashes: Arc<Vec<Mutex<Option<crate::artifact::hash::ChunkHash>>>>,
    output_handles: Arc<OutputHandleCache>,
    compression_profile: Arc<crate::manifest::CompressionProfile>,
    control: GutchOperationControl,
    reconstruct_worker_count: usize,
}

fn spawn_reconstruct_work(
    pool: &ThreadPool,
    work_item: ReconstructWorkItem,
    context: &Arc<ReconstructWorkContext>,
    event_tx: &mpsc::Sender<ReconstructJobEvent>,
    memory_permit: crate::runtime::MemoryPermit,
    queued_at: ProfileInstant,
) {
    let context = Arc::clone(context);
    let event_tx = event_tx.clone();
    pool.spawn(move || {
        let queue_wait = queued_at.elapsed();
        let _work_span =
            context
                .control
                .profile_span_with_args("installation.worker.execute", || {
                    std::collections::BTreeMap::from([
                        (
                            "queue_wait_us".into(),
                            serde_json::json!(queue_wait.as_micros() as u64),
                        ),
                        (
                            "work_kind".into(),
                            serde_json::json!(match &work_item {
                                ReconstructWorkItem::Job(_) => "reconstruct",
                                ReconstructWorkItem::VerifyFile(_) => "verify",
                            }),
                        ),
                    ])
                });
        let result = catch_unwind(AssertUnwindSafe(|| match work_item {
            ReconstructWorkItem::Job(job_index) => {
                reconstruct_job(super::io::ReconstructJobRequest {
                    job: &context.jobs[job_index],
                    file_plans: &context.file_plans,
                    output_paths: &context.output_paths,
                    precomputed_file_hashes: &context.precomputed_file_hashes,
                    output_handles: &context.output_handles,
                    source_handles: &context.source_handles,
                    compression_profile: &context.compression_profile,
                    control: &context.control,
                })
                .map(|timings| ReconstructJobEvent::Completed {
                    job_index,
                    timings: Box::new(timings),
                })
            }
            ReconstructWorkItem::VerifyFile(file_index) => verify_reconstructed_output_file(
                file_index,
                &context.file_plans[file_index],
                &context.output_paths,
                &context.precomputed_file_hashes,
                &context.control,
                context.reconstruct_worker_count,
            )
            .map(ReconstructJobEvent::Verified),
        }));
        let result = match result {
            Ok(result) => result,
            Err(_) => Err(Error::reconstruct("reconstruction worker panicked")),
        };
        // Completion lets the coordinator finalize profiling; close the worker span first.
        drop(memory_permit);
        drop(_work_span);
        // Publication may follow the final event immediately. Release cached file
        // handles before allowing the coordinator to advance to that boundary.
        drop(context);
        match result {
            Ok(event) => {
                let _ = event_tx.send(event);
            }
            Err(error) => {
                let _ = event_tx.send(ReconstructJobEvent::Failed(error));
            }
        }
    });
}

pub(crate) fn run_reconstruction_jobs(
    request: ExecuteReconstructPipelineRequest<'_>,
) -> Result<(
    ReconstructTimings,
    crate::installation::inspect::VerifyTimings,
)> {
    let ExecuteReconstructPipelineRequest {
        pipeline_plan,
        output_paths,
        snapshot_id,
        compression_profile,
        runtime_hints,
        existing_artifacts,
        pending_artifact_downloads,
        transfer_policy,
        control,
        download_client,
        transfer_progress,
        reconstruct_pool,
    } = request;

    control.checkpoint()?;
    let _pipeline_span = control.profile_span("installation.reconstruct.pipeline");
    let pipeline_started_at = ProfileInstant::start(control.profiler());
    let runtime_plan = plan_runtime(runtime_hints);
    control.profile_event_with_args("runtime.memory.budget", || {
        std::collections::BTreeMap::from([(
            "budget_bytes".into(),
            serde_json::json!(runtime_plan.memory_budget_bytes),
        )])
    });
    let mut reconstruct_timings = ReconstructTimings {
        runtime_plan: Some(runtime_plan.clone()),
        verify_files: true,
        ..ReconstructTimings::default()
    };
    let mut verify_timings = crate::installation::inspect::VerifyTimings {
        verify_mode: crate::installation::inspect::VerifyMode::Fast,
        runtime_plan: Some(plan_runtime(runtime_hints)),
        ..crate::installation::inspect::VerifyTimings::default()
    };

    let artifact_priorities = {
        let _priorities_span = control.profile_span("installation.reconstruct.prepare.priorities");
        build_artifact_priorities(&pipeline_plan, pending_artifact_downloads)
    };
    let file_plans = Arc::new(pipeline_plan.files);
    let jobs = Arc::new(pipeline_plan.jobs);
    let output_sizes = Arc::new(file_plans.iter().map(|file| file.size).collect::<Vec<_>>());
    let precomputed_file_hashes = Arc::new(
        (0..file_plans.len())
            .map(|_| Mutex::new(None))
            .collect::<Vec<Mutex<Option<crate::artifact::hash::ChunkHash>>>>(),
    );
    let (artifact_job_indices, artifact_scheduler) = {
        let _artifacts_span = control.profile_span("installation.reconstruct.prepare.artifacts");
        let pending_artifacts = pending_artifact_downloads
            .iter()
            .map(|download| download.artifact.clone())
            .collect::<HashSet<_>>();
        let mut artifact_job_indices = HashMap::new();
        for (job_index, job) in jobs.iter().enumerate() {
            let ReconstructJob::Artifact(job) = job else {
                continue;
            };
            artifact_job_indices.insert(job.artifact.clone(), job_index);
            if !existing_artifacts.contains(&job.artifact)
                && !pending_artifacts.contains(&job.artifact)
            {
                return Err(Error::reconstruct(format!(
                    "required artifact '{}' is neither local nor scheduled for download",
                    job.artifact.id()
                )));
            }
            if existing_artifacts.contains(&job.artifact) && !job.artifact_path.is_file() {
                return Err(Error::reconstruct(format!(
                    "required artifact '{}' is not available at {}",
                    job.artifact.id(),
                    job.artifact_path.display()
                )));
            }
        }
        let artifact_scheduler = if pending_artifact_downloads.is_empty() {
            None
        } else {
            let Some(download_client) = download_client else {
                return Err(Error::reconstruct(
                    "bundled reconstruction requires a download client for pending bundles",
                ));
            };
            let Some(transfer_progress) = transfer_progress else {
                return Err(Error::reconstruct(
                    "bundled reconstruction requires transfer progress for pending bundles",
                ));
            };
            Some(ArtifactDownloadScheduler::new(
                pending_artifact_downloads,
                artifact_priorities,
                transfer_policy,
                download_client,
                control,
                Arc::clone(transfer_progress),
            )?)
        };
        (artifact_job_indices, artifact_scheduler)
    };
    let artifact_worker_count = artifact_scheduler
        .as_ref()
        .map(ArtifactDownloadScheduler::worker_count)
        .unwrap_or(0);

    emit_progress(
        control.progress(),
        GutchProgressEvent {
            feedback: None,
            operation: GutchOperation::Fetch,
            phase: GutchProgressPhase::Reconstruct,
            message: format!("Preparing files (0/{})", file_plans.len().max(1)),
            current: 0,
            total: file_plans.len().max(1),
            transferred_bytes: 0,
            total_bytes: 0,
            detail: Some(snapshot_id.to_string()),
            done: false,
        },
    );

    let (event_tx, event_rx) = mpsc::channel::<ReconstructJobEvent>();
    let compression_profile = Arc::new((*compression_profile).clone());
    // Keep producer accounting after download startup so handle selection does
    // not delay the existing overlap between transfer and preparation.
    let mut remaining_file_jobs = vec![0usize; file_plans.len()];
    let job_file_indices = jobs
        .iter()
        .map(reconstruct_job_file_indices)
        .collect::<Vec<_>>();
    for file_indices in &job_file_indices {
        for &file_index in file_indices {
            if let Some(remaining) = remaining_file_jobs.get_mut(file_index) {
                *remaining += 1;
            }
        }
    }
    let output_handles = Arc::new(OutputHandleCache::new(
        Arc::clone(&output_paths),
        Arc::clone(&output_sizes),
        &remaining_file_jobs,
        runtime_plan.worker_count,
    ));
    let work_context = Arc::new(ReconstructWorkContext {
        memory: Arc::new(crate::runtime::MemoryBudget::new(
            runtime_plan.memory_budget_bytes,
        )),
        source_handles: SourceHandleCache::default(),
        jobs: Arc::clone(&jobs),
        file_plans: Arc::clone(&file_plans),
        output_paths: Arc::clone(&output_paths),
        precomputed_file_hashes,
        output_handles,
        compression_profile,
        control: control.clone(),
        reconstruct_worker_count: runtime_plan.worker_count,
    });
    let mut work_queue = ReconstructionWorkQueue::default();
    let mut enqueued_job_count = 0usize;
    let mut downloads_done = artifact_scheduler.is_none();
    let mut completed_download_worker_count = 0usize;

    let mut completed_job_count = 0usize;
    let mut completed_file_count = 0usize;
    let mut scheduled_job_indices = HashSet::new();
    let mut verification_queued = vec![false; file_plans.len()];
    let mut verify_started_at = None;
    let mut verify_finished_at = None;
    let mut verify_span = None;
    let mut verified_file_bytes = 0u64;
    let mut pipeline_error = None;
    let reconstruct_started_at = ProfileInstant::start(control.profiler());
    let mut reconstruct_finished_at = jobs.is_empty().then_some(reconstruct_started_at);
    if jobs.is_empty() {
        control.output_finished();
    }
    reconstruct_timings.prepare_wall = pipeline_started_at.elapsed();

    let pool = match reconstruct_pool {
        Some(pool) => pool,
        None if jobs.is_empty() && file_plans.is_empty() => {
            reconstruct_timings.total = pipeline_started_at.elapsed();
            return Ok((reconstruct_timings, verify_timings));
        }
        None => {
            return Err(Error::reconstruct(
                "reconstruction thread pool is unavailable",
            ));
        }
    };
    {
        let _dispatch_span = control.profile_span("installation.reconstruct.dispatch.ready");
        for (job_index, job) in jobs.iter().enumerate() {
            let ready = match job {
                ReconstructJob::Artifact(job) => existing_artifacts.contains(&job.artifact),
                ReconstructJob::SourceChunk(_) | ReconstructJob::CopySourceFile { .. } => true,
            };
            if ready {
                work_queue.ready.push_back((
                    ReconstructWorkItem::Job(job_index),
                    ProfileInstant::detailed(control.profiler()),
                ));
                scheduled_job_indices.insert(job_index);

                enqueued_job_count += 1;
            }
        }
    }
    update_reconstruct_backpressure(
        transfer_progress,
        enqueued_job_count,
        completed_job_count,
        runtime_plan.worker_count,
    );
    for file_index in 0..file_plans.len() {
        if remaining_file_jobs[file_index] == 0 {
            ensure_empty_reconstruct_output_file(
                &file_plans[file_index],
                &output_paths[file_index],
                control,
            )?;
            work_queue.ready.push_back((
                ReconstructWorkItem::VerifyFile(file_index),
                ProfileInstant::detailed(control.profiler()),
            ));
            verification_queued[file_index] = true;

            if verify_started_at.is_none() {
                verify_started_at = Some(ProfileInstant::start(control.profiler()));
                verify_span = control.profile_span("installation.reconstruct.verify");
            }
        }
    }
    while completed_job_count < jobs.len() || completed_file_count < file_plans.len() {
        if let Err(error) = control.checkpoint() {
            pipeline_error = Some(error);
            break;
        }
        if !downloads_done && let Some(scheduler) = artifact_scheduler.as_ref() {
            while !downloads_done {
                match scheduler.try_recv() {
                    Ok(ArtifactDownloadEvent::Ready(artifact)) => {
                        let Some(&job_index) = artifact_job_indices.get(&artifact) else {
                            pipeline_error = Some(Error::reconstruct(format!(
                                "downloaded unexpected artifact '{}'",
                                artifact.id()
                            )));
                            break;
                        };
                        let ReconstructJob::Artifact(job) = &jobs[job_index] else {
                            pipeline_error = Some(Error::reconstruct(format!(
                                "downloaded artifact '{}' is not an artifact job",
                                artifact.id()
                            )));
                            break;
                        };
                        if !job.artifact_path.is_file() {
                            pipeline_error = Some(Error::reconstruct(format!(
                                "downloaded artifact '{}' is missing at {}",
                                artifact.id(),
                                job.artifact_path.display()
                            )));
                            break;
                        }
                        if scheduled_job_indices.insert(job_index) {
                            let _dispatch_span = control
                                .profile_span("installation.reconstruct.dispatch.downloaded");
                            work_queue.ready.push_back((
                                ReconstructWorkItem::Job(job_index),
                                ProfileInstant::detailed(control.profiler()),
                            ));

                            enqueued_job_count += 1;
                            update_reconstruct_backpressure(
                                transfer_progress,
                                enqueued_job_count,
                                completed_job_count,
                                runtime_plan.worker_count,
                            );
                        }
                    }
                    Ok(ArtifactDownloadEvent::Failed(error)) => {
                        pipeline_error = Some(error);
                        break;
                    }
                    Ok(ArtifactDownloadEvent::WorkerDone) => {
                        completed_download_worker_count += 1;
                        if completed_download_worker_count == artifact_worker_count {
                            downloads_done = true;
                            if let Some(transfer_progress) = transfer_progress {
                                transfer_progress.finish();
                                control.download_finished();
                            }
                        }
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        pipeline_error = Some(Error::reconstruct(format!(
                            "artifact download scheduler channel closed unexpectedly after scheduling {}/{} artifact jobs; {completed_download_worker_count}/{artifact_worker_count} workers reported completion",
                            scheduled_job_indices.len(),
                            artifact_job_indices.len(),
                        )));
                        break;
                    }
                }
            }
        }

        // Admit only runnable jobs, leaving pool threads free instead of blocking them
        // on memory. Verification can proceed while larger artifact jobs wait.
        while pipeline_error.is_none() && work_queue.in_flight < runtime_plan.worker_count {
            match work_queue.admit_next(&jobs, &work_context.memory) {
                Ok(Some(admitted)) => {
                    spawn_reconstruct_work(
                        pool,
                        admitted.work,
                        &work_context,
                        &event_tx,
                        admitted.permit,
                        admitted.queued_at,
                    );
                }
                Ok(None) => break,
                Err(error) => pipeline_error = Some(error),
            }
        }
        if pipeline_error.is_some() {
            break;
        }
        if completed_job_count == jobs.len() && completed_file_count == file_plans.len() {
            break;
        }
        let event = {
            let _wait_span = control.profile_span("installation.coordinator.wait_for_worker");
            event_rx.recv_timeout(Duration::from_millis(10))
        };
        match event {
            Ok(ReconstructJobEvent::Completed {
                job_index,
                timings: job_timings,
            }) => {
                work_queue.complete();
                completed_job_count += 1;
                update_reconstruct_backpressure(
                    transfer_progress,
                    enqueued_job_count,
                    completed_job_count,
                    runtime_plan.worker_count,
                );
                accumulate_pipeline_timings(
                    &mut reconstruct_timings,
                    &mut verify_timings,
                    &job_timings,
                );
                emit_progress(
                    control.progress(),
                    GutchProgressEvent {
                        feedback: None,
                        operation: GutchOperation::Fetch,
                        phase: GutchProgressPhase::Reconstruct,
                        message: format!(
                            "Reconstructing jobs ({}/{})",
                            completed_job_count,
                            jobs.len().max(1)
                        ),
                        current: completed_job_count,
                        total: jobs.len().max(1),
                        transferred_bytes: 0,
                        total_bytes: 0,
                        detail: None,
                        done: false,
                    },
                );
                if reconstruct_finished_at.is_none() && completed_job_count == jobs.len() {
                    reconstruct_finished_at = Some(ProfileInstant::start(control.profiler()));
                    control.output_finished();
                }
                for &file_index in &job_file_indices[job_index] {
                    remaining_file_jobs[file_index] =
                        remaining_file_jobs[file_index].saturating_sub(1);
                    if remaining_file_jobs[file_index] == 0 && !verification_queued[file_index] {
                        if verify_started_at.is_none() {
                            verify_started_at = Some(ProfileInstant::start(control.profiler()));
                            verify_span = control.profile_span("installation.reconstruct.verify");
                        }
                        // Verify while the completed file may still be cached, overlapping
                        // hashing with remaining writes. All its producers have finished.
                        work_queue.ready.push_front((
                            ReconstructWorkItem::VerifyFile(file_index),
                            ProfileInstant::detailed(control.profiler()),
                        ));
                        verification_queued[file_index] = true;
                    }
                }
            }
            Ok(ReconstructJobEvent::Verified(file_verification)) => {
                work_queue.complete();
                completed_file_count += 1;
                if verify_finished_at.is_none() && completed_file_count == file_plans.len() {
                    verify_finished_at = Some(ProfileInstant::start(control.profiler()));
                }
                reconstruct_timings.file_verify += file_verification.elapsed;
                reconstruct_timings.file_verify_bytes = reconstruct_timings
                    .file_verify_bytes
                    .saturating_add(file_verification.bytes);
                verified_file_bytes = verified_file_bytes.saturating_add(file_verification.bytes);
                emit_progress(
                    control.progress(),
                    GutchProgressEvent {
                        feedback: None,
                        operation: GutchOperation::Fetch,
                        phase: GutchProgressPhase::Reconstruct,
                        message: format!(
                            "Reconstructing files ({}/{})",
                            completed_file_count,
                            file_plans.len().max(1)
                        ),
                        current: completed_file_count,
                        total: file_plans.len().max(1),
                        transferred_bytes: 0,
                        total_bytes: 0,
                        detail: None,
                        done: false,
                    },
                );
            }
            Ok(ReconstructJobEvent::Failed(error)) => {
                work_queue.complete();
                pipeline_error = Some(error);
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                pipeline_error = Some(Error::reconstruct(
                    "reconstruction job event channel closed unexpectedly",
                ));
                break;
            }
        }
    }
    if pipeline_error.is_some()
        && let Some(scheduler) = artifact_scheduler.as_ref()
    {
        scheduler.cancel();
    }
    if let Some(scheduler) = artifact_scheduler {
        scheduler.join()?;
    }
    // Queued work has never started and emits no completion event.
    work_queue.ready.clear();
    while pipeline_error.is_some() && work_queue.in_flight > 0 {
        let event = {
            let _wait_span = control.profile_span("installation.coordinator.drain_workers");
            event_rx.recv()
        };
        match event {
            Ok(_) => work_queue.complete(),
            Err(_) => break,
        }
    }
    if let Some(error) = pipeline_error {
        return Err(error);
    }
    if completed_job_count != jobs.len() {
        return Err(Error::reconstruct(format!(
            "reconstruction completed {completed_job_count} of {} jobs",
            jobs.len()
        )));
    }
    if completed_file_count != file_plans.len() {
        return Err(Error::reconstruct(format!(
            "verification completed {completed_file_count} of {} files",
            file_plans.len()
        )));
    }

    let verify_elapsed = verify_started_at
        .zip(verify_finished_at)
        .map(|(started_at, finished_at)| finished_at.saturating_duration_since(started_at))
        .unwrap_or_default();
    drop(verify_span);
    verify_timings.file_hash += verify_elapsed;
    verify_timings.file_hash_bytes = verify_timings
        .file_hash_bytes
        .saturating_add(verified_file_bytes);
    emit_progress(
        control.progress(),
        GutchProgressEvent {
            feedback: None,
            operation: GutchOperation::Fetch,
            phase: GutchProgressPhase::Reconstruct,
            message: format!(
                "Reconstructing files ({}/{})",
                file_plans.len(),
                file_plans.len().max(1)
            ),
            current: file_plans.len(),
            total: file_plans.len().max(1),
            transferred_bytes: 0,
            total_bytes: 0,
            detail: None,
            done: false,
        },
    );

    reconstruct_timings.reconstruct_wall = reconstruct_finished_at
        .map(|finished_at| finished_at.saturating_duration_since(reconstruct_started_at))
        .unwrap_or_default();
    reconstruct_timings.total = pipeline_started_at.elapsed();
    verify_timings.verify_files_wall = verify_elapsed;
    verify_timings.total = verify_elapsed;
    Ok((reconstruct_timings, verify_timings))
}

fn reconstruct_job_file_indices(job: &ReconstructJob) -> Vec<usize> {
    let mut file_indices = match job {
        ReconstructJob::Artifact(job) => job
            .chunks
            .iter()
            .flat_map(|chunk| {
                job.targets[chunk.target_range.clone()]
                    .iter()
                    .map(|target| target.file_index)
            })
            .collect::<Vec<_>>(),
        ReconstructJob::SourceChunk(job) => job
            .targets
            .iter()
            .map(|target| target.file_index)
            .collect::<Vec<_>>(),
        ReconstructJob::CopySourceFile { file_index, .. } => vec![*file_index],
    };
    file_indices.sort_unstable();
    file_indices.dedup();
    file_indices
}

fn update_reconstruct_backpressure(
    transfer_progress: Option<&Arc<FetchTransferProgress>>,
    enqueued_job_count: usize,
    completed_job_count: usize,
    worker_count: usize,
) {
    let Some(transfer_progress) = transfer_progress else {
        return;
    };
    let queue_limit = worker_count.saturating_mul(2).max(1);
    let queued_jobs = enqueued_job_count.saturating_sub(completed_job_count);
    transfer_progress.set_downstream_backpressure(queued_jobs > queue_limit);
}

fn ensure_empty_reconstruct_output_file(
    file_plan: &ReconstructPipelineFile,
    output_path: &Path,
    control: &GutchOperationControl,
) -> Result<()> {
    let _file_span = control
        .profile_span_with_args("installation.reconstruct.prepare.file", || {
            crate::profile::file_trace_args(file_plan.path.clone(), file_plan.size)
        });
    let output = {
        let _create_span = control
            .profile_span_with_args("installation.reconstruct.prepare.file.create", || {
                crate::profile::file_trace_args(file_plan.path.clone(), file_plan.size)
            });
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(output_path)
            .map_err(|e| Error::io_context("creating empty output file", output_path, e))?
    };
    {
        let _size_span = control
            .profile_span_with_args("installation.reconstruct.prepare.file.set_len", || {
                crate::profile::file_trace_args(file_plan.path.clone(), file_plan.size)
            });
        output
            .set_len(file_plan.size)
            .map_err(|e| Error::io_context("sizing empty output file", output_path, e))?;
    }
    Ok(())
}

fn accumulate_pipeline_timings(
    reconstruct_timings: &mut ReconstructTimings,
    verify_timings: &mut crate::installation::inspect::VerifyTimings,
    file_timings: &ReconstructTimings,
) {
    reconstruct_timings.artifact_read += file_timings.artifact_read;
    reconstruct_timings.artifact_read_compressed_bytes +=
        file_timings.artifact_read_compressed_bytes;
    reconstruct_timings.artifact_read_decompressed_bytes +=
        file_timings.artifact_read_decompressed_bytes;
    reconstruct_timings.bundle_read += file_timings.bundle_read;
    reconstruct_timings.bundle_read_compressed_bytes += file_timings.bundle_read_compressed_bytes;
    reconstruct_timings.bundle_read_decompressed_bytes +=
        file_timings.bundle_read_decompressed_bytes;
    reconstruct_timings.chunk_verify += file_timings.chunk_verify;
    reconstruct_timings.chunk_verify_bytes += file_timings.chunk_verify_bytes;
    reconstruct_timings.file_rebuild += file_timings.file_rebuild;
    reconstruct_timings.file_rebuild_bytes += file_timings.file_rebuild_bytes;
    reconstruct_timings.file_verify += file_timings.file_verify;
    reconstruct_timings.file_verify_bytes += file_timings.file_verify_bytes;
    reconstruct_timings.file_write += file_timings.file_write;
    reconstruct_timings.file_write_bytes += file_timings.file_write_bytes;

    verify_timings.file_hash += file_timings.file_verify;
    verify_timings.file_hash_bytes += file_timings.file_verify_bytes;
}

#[cfg(test)]
#[path = "execute_tests.rs"]
mod tests;

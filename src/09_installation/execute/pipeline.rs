//! Coordinates artifact readiness, job dispatch and file verification for one reconstruction.

use crate::profiling::ProfileInstant;
use crate::profiling::ProfileSpan;
use std::collections::HashMap;
use std::collections::HashSet;
use std::fs::OpenOptions;
use std::ops::Range;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use rayon::ThreadPool;

use crate::Error;
use crate::Result;
use crate::installation::contracts::ReconstructTimings;
use crate::progress::OperationControl;
use crate::progress::OperationKind;
use crate::progress::ProgressEvent;
use crate::progress::ProgressPhase;
use crate::progress::emit_progress;
use crate::runtime::RuntimeResourceHints;
use crate::runtime::plan_runtime;
use crate::transfer::download::ArtifactDownloadEvent;
use crate::transfer::download::ArtifactDownloadScheduler;
use crate::transfer::download::PendingArtifactDownload;
use crate::transfer::progress::FetchTransferProgress;

use super::handles::{OutputHandleCache, SourceHandleCache};
use super::job_io::ReconstructedFileVerification;
use super::jobs::{ReconstructJob, ReconstructPipelineFile, ReconstructPipelinePlan};
use crate::installation::plan::build_artifact_priorities;

use super::workers::{
    ReconstructJobEvent, ReconstructWorkContext, ReconstructWorkItem, ReconstructWorkQueue,
    dispatch_ready_work,
};
use crate::artifact::profiles::CompressionProfile;
use crate::artifact::reference::ArtifactRef;
use crate::installation::inspect::VerifyMode;
use crate::installation::inspect::VerifyTimings;
use crate::profiling::file_trace_args;
use crate::runtime::MemoryBudget;
use crate::transfer::download::ArtifactPriority;
use crate::transfer::http::DownloadClient;
use crate::transfer::types::TransferConcurrencyPolicy;
use std::collections::BTreeMap;
pub(crate) struct ExecuteReconstructPipelineRequest<'a> {
    pub(crate) pipeline_plan: ReconstructPipelinePlan,
    pub(crate) output_paths: Arc<Vec<PathBuf>>,
    pub(crate) snapshot_id: &'a str,
    pub(crate) compression_profile: &'a CompressionProfile,
    pub(crate) runtime_hints: &'a RuntimeResourceHints,
    pub(crate) existing_artifacts: &'a HashSet<ArtifactRef>,
    pub(crate) pending_artifact_downloads: &'a [PendingArtifactDownload],
    pub(crate) transfer_policy: TransferConcurrencyPolicy,
    pub(crate) control: &'a OperationControl,
    pub(crate) download_client: Option<&'a DownloadClient>,
    pub(crate) transfer_progress: Option<&'a Arc<FetchTransferProgress>>,
    pub(crate) reconstruct_pool: Option<&'a ThreadPool>,
}

pub(crate) fn run_reconstruction_jobs(
    request: ExecuteReconstructPipelineRequest<'_>,
) -> Result<(ReconstructTimings, VerifyTimings)> {
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
        BTreeMap::from([(
            "budget_bytes".into(),
            serde_json::json!(runtime_plan.memory_budget_bytes),
        )])
    });
    let mut reconstruct_timings = ReconstructTimings {
        runtime_plan: Some(runtime_plan.clone()),
        verify_files: true,
        ..ReconstructTimings::default()
    };
    let verify_timings = VerifyTimings {
        verify_mode: VerifyMode::Fast,
        runtime_plan: Some(plan_runtime(runtime_hints)),
        ..VerifyTimings::default()
    };

    let artifact_priorities = {
        let _priorities_span = control.profile_span("installation.reconstruct.prepare.priorities");
        build_artifact_priorities(&pipeline_plan, pending_artifact_downloads)
    };
    let file_plans = Arc::new(pipeline_plan.files);
    let jobs = Arc::new(pipeline_plan.jobs);
    let output_sizes = Arc::new(file_plans.iter().map(|file| file.size).collect::<Vec<_>>());
    let (artifact_job_indices, artifact_scheduler) = {
        let _artifacts_span = control.profile_span("installation.reconstruct.prepare.artifacts");
        let artifact_job_indices =
            validate_artifact_jobs(&jobs, existing_artifacts, pending_artifact_downloads)?;
        let artifact_scheduler = start_artifact_downloads(
            pending_artifact_downloads,
            artifact_priorities,
            transfer_policy,
            download_client,
            control,
            transfer_progress,
        )?;
        (artifact_job_indices, artifact_scheduler)
    };
    emit_reconstruct_progress(
        control,
        "Preparing files",
        0,
        file_plans.len(),
        Some(snapshot_id),
    );

    let (event_tx, event_rx) = mpsc::channel::<ReconstructJobEvent>();
    // Keep producer accounting after download startup so handle selection does
    // not delay the existing overlap between transfer and preparation.
    let job_file_indices = jobs
        .iter()
        .map(reconstruct_job_file_indices)
        .collect::<Vec<_>>();
    let remaining_file_jobs = count_jobs_per_file(&job_file_indices, file_plans.len());
    let output_handles = Arc::new(OutputHandleCache::new(
        Arc::clone(&output_paths),
        Arc::clone(&output_sizes),
        &remaining_file_jobs,
        runtime_plan.worker_count,
    ));
    let work_context = Arc::new(ReconstructWorkContext {
        memory: Arc::new(MemoryBudget::new(runtime_plan.memory_budget_bytes)),
        source_handles: SourceHandleCache::default(),
        jobs: Arc::clone(&jobs),
        file_plans: Arc::clone(&file_plans),
        output_paths: Arc::clone(&output_paths),
        output_handles,
        compression_profile: Arc::new(compression_profile.clone()),
        control: control.clone(),
        reconstruct_worker_count: runtime_plan.worker_count,
    });
    let reconstruct_started_at = ProfileInstant::start(control.profiler());
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

    let mut coordinator = PipelineCoordinator {
        control,
        transfer_progress,
        jobs: &jobs,
        file_plans: &file_plans,
        output_paths: &output_paths,
        job_file_indices: &job_file_indices,
        worker_count: runtime_plan.worker_count,
        downloads_done: artifact_scheduler.is_none(),
        artifact_worker_count: artifact_scheduler
            .as_ref()
            .map(ArtifactDownloadScheduler::worker_count)
            .unwrap_or(0),
        finished_download_workers: 0,
        artifact_scheduler,
        artifact_job_indices,
        work_queue: ReconstructWorkQueue::default(),
        scheduled_jobs: vec![false; jobs.len()],
        remaining_file_jobs,
        verification_queued: vec![false; file_plans.len()],
        enqueued_job_count: 0,
        completed_job_count: 0,
        completed_file_count: 0,
        reconstruct_started_at,
        reconstruct_finished_at: jobs.is_empty().then_some(reconstruct_started_at),
        verify_started_at: None,
        verify_finished_at: None,
        verify_span: None,
        verified_file_bytes: 0,
        reconstruct_timings,
        verify_timings,
    };
    coordinator.queue_initial_work(existing_artifacts)?;
    let result = coordinator.run(pool, &work_context, &event_tx, &event_rx);
    finish_pipeline_workers(
        coordinator.artifact_scheduler.take(),
        &mut coordinator.work_queue,
        &event_rx,
        control,
        result.is_err(),
    )?;
    result?;
    coordinator.finish(pipeline_started_at)
}

/// Tracks one reconstruction run: which jobs and verifications are queued,
/// which artifacts arrived, and how far jobs and files have progressed.
struct PipelineCoordinator<'a> {
    control: &'a OperationControl,
    transfer_progress: Option<&'a Arc<FetchTransferProgress>>,
    jobs: &'a [ReconstructJob],
    file_plans: &'a [ReconstructPipelineFile],
    output_paths: &'a [PathBuf],
    /// Output files each job writes, by job index.
    job_file_indices: &'a [Vec<usize>],
    worker_count: usize,

    artifact_scheduler: Option<ArtifactDownloadScheduler>,
    artifact_job_indices: HashMap<ArtifactRef, usize>,
    artifact_worker_count: usize,
    finished_download_workers: usize,
    downloads_done: bool,

    work_queue: ReconstructWorkQueue,
    scheduled_jobs: Vec<bool>,
    /// Jobs still to finish writing each output file, by file index.
    remaining_file_jobs: Vec<usize>,
    verification_queued: Vec<bool>,
    enqueued_job_count: usize,
    completed_job_count: usize,
    completed_file_count: usize,

    reconstruct_started_at: ProfileInstant,
    reconstruct_finished_at: Option<ProfileInstant>,
    verify_started_at: Option<ProfileInstant>,
    verify_finished_at: Option<ProfileInstant>,
    verify_span: Option<ProfileSpan>,
    verified_file_bytes: u64,
    reconstruct_timings: ReconstructTimings,
    verify_timings: VerifyTimings,
}

impl PipelineCoordinator<'_> {
    /// Queues jobs whose inputs are already local, then the verification of
    /// files that no job writes.
    fn queue_initial_work(&mut self, existing_artifacts: &HashSet<ArtifactRef>) -> Result<()> {
        {
            let _dispatch_span = self
                .control
                .profile_span("installation.reconstruct.dispatch.ready");
            let jobs = self.jobs;
            for (job_index, job) in jobs.iter().enumerate() {
                let ready = match job {
                    ReconstructJob::Artifact(job) => existing_artifacts.contains(&job.artifact),
                    ReconstructJob::SourceChunk(_) | ReconstructJob::CopySourceFile { .. } => true,
                };
                if ready {
                    self.queue_job(job_index);
                }
            }
        }
        self.update_backpressure();
        for file_index in 0..self.file_plans.len() {
            if self.remaining_file_jobs[file_index] == 0 {
                ensure_empty_reconstruct_output_file(
                    &self.file_plans[file_index],
                    &self.output_paths[file_index],
                    self.control,
                )?;
                let work = self.start_verification(file_index);
                self.work_queue.ready.push_back(work);
            }
        }
        Ok(())
    }

    /// Dispatches work and handles download and worker events until every job
    /// and file verification has completed.
    fn run(
        &mut self,
        pool: &ThreadPool,
        work_context: &Arc<ReconstructWorkContext>,
        event_tx: &mpsc::Sender<ReconstructJobEvent>,
        event_rx: &mpsc::Receiver<ReconstructJobEvent>,
    ) -> Result<()> {
        while !self.is_complete() {
            self.control.checkpoint()?;
            self.receive_download_events()?;
            dispatch_ready_work(pool, &mut self.work_queue, work_context, event_tx)?;
            if self.is_complete() {
                break;
            }
            let event = {
                let _wait_span = self
                    .control
                    .profile_span("installation.coordinator.wait_for_worker");
                event_rx.recv_timeout(Duration::from_millis(10))
            };
            match event {
                Ok(ReconstructJobEvent::Completed {
                    job_indices,
                    timings,
                }) => self.complete_jobs(job_indices, &timings),
                Ok(ReconstructJobEvent::Verified {
                    file_count,
                    timings,
                }) => self.complete_verification(file_count, timings),
                Ok(ReconstructJobEvent::Failed(error)) => {
                    self.work_queue.complete();
                    return Err(error);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(Error::reconstruct(
                        "reconstruction job event channel closed unexpectedly",
                    ));
                }
            }
        }
        Ok(())
    }

    fn is_complete(&self) -> bool {
        self.completed_job_count == self.jobs.len()
            && self.completed_file_count == self.file_plans.len()
    }

    /// Queues the jobs whose artifacts finished downloading since the last call.
    fn receive_download_events(&mut self) -> Result<()> {
        while !self.downloads_done {
            let Some(scheduler) = self.artifact_scheduler.as_ref() else {
                return Ok(());
            };
            match scheduler.try_recv() {
                Ok(ArtifactDownloadEvent::Ready(artifact)) => self.artifact_ready(&artifact)?,
                Ok(ArtifactDownloadEvent::Failed(error)) => return Err(error),
                Ok(ArtifactDownloadEvent::WorkerDone) => self.download_worker_done(),
                Err(mpsc::TryRecvError::Empty) => return Ok(()),
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Err(Error::reconstruct(format!(
                        "artifact download scheduler channel closed unexpectedly after scheduling {}/{} artifact jobs; {}/{} workers reported completion",
                        self.enqueued_job_count,
                        self.artifact_job_indices.len(),
                        self.finished_download_workers,
                        self.artifact_worker_count,
                    )));
                }
            }
        }
        Ok(())
    }

    fn artifact_ready(&mut self, artifact: &ArtifactRef) -> Result<()> {
        let Some(&job_index) = self.artifact_job_indices.get(artifact) else {
            return Err(Error::reconstruct(format!(
                "downloaded unexpected artifact '{}'",
                artifact.id()
            )));
        };
        let ReconstructJob::Artifact(job) = &self.jobs[job_index] else {
            return Err(Error::reconstruct(format!(
                "downloaded artifact '{}' is not an artifact job",
                artifact.id()
            )));
        };
        if !job.artifact_path.is_file() {
            return Err(Error::reconstruct(format!(
                "downloaded artifact '{}' is missing at {}",
                artifact.id(),
                job.artifact_path.display()
            )));
        }
        if !self.scheduled_jobs[job_index] {
            let _dispatch_span = self
                .control
                .profile_span("installation.reconstruct.dispatch.downloaded");
            self.queue_job(job_index);
            self.update_backpressure();
        }
        Ok(())
    }

    fn download_worker_done(&mut self) {
        self.finished_download_workers += 1;
        if self.finished_download_workers < self.artifact_worker_count {
            return;
        }
        self.downloads_done = true;
        if let Some(transfer_progress) = self.transfer_progress {
            transfer_progress.finish();
            self.control.download_finished();
        }
    }

    fn complete_jobs(&mut self, job_indices: Range<usize>, job_timings: &[ReconstructTimings]) {
        self.work_queue.complete();
        self.completed_job_count += job_indices.len();
        self.update_backpressure();
        for timings in job_timings {
            accumulate_pipeline_timings(
                &mut self.reconstruct_timings,
                &mut self.verify_timings,
                timings,
            );
        }
        emit_reconstruct_progress(
            self.control,
            "Reconstructing jobs",
            self.completed_job_count,
            self.jobs.len(),
            None,
        );
        if self.reconstruct_finished_at.is_none() && self.completed_job_count == self.jobs.len() {
            self.reconstruct_finished_at = Some(ProfileInstant::start(self.control.profiler()));
            self.control.output_finished();
        }
        let job_file_indices = self.job_file_indices;
        for &file_index in job_indices.flat_map(|job_index| job_file_indices[job_index].iter()) {
            let remaining = &mut self.remaining_file_jobs[file_index];
            *remaining = remaining.saturating_sub(1);
            if *remaining == 0 && !self.verification_queued[file_index] {
                // Verify while the completed file may still be cached, overlapping
                // hashing with remaining writes. All its producers have finished.
                let work = self.start_verification(file_index);
                self.work_queue.ready.push_front(work);
            }
        }
    }

    fn complete_verification(
        &mut self,
        file_count: usize,
        verification: ReconstructedFileVerification,
    ) {
        self.work_queue.complete();
        self.completed_file_count += file_count;
        if self.verify_finished_at.is_none() && self.completed_file_count == self.file_plans.len() {
            self.verify_finished_at = Some(ProfileInstant::start(self.control.profiler()));
        }
        self.reconstruct_timings.file_verify += verification.elapsed;
        self.reconstruct_timings.file_verify_bytes = self
            .reconstruct_timings
            .file_verify_bytes
            .saturating_add(verification.bytes);
        self.verified_file_bytes = self.verified_file_bytes.saturating_add(verification.bytes);
        emit_reconstruct_progress(
            self.control,
            "Reconstructing files",
            self.completed_file_count,
            self.file_plans.len(),
            None,
        );
    }

    fn queue_job(&mut self, job_index: usize) {
        self.work_queue.ready.push_back((
            ReconstructWorkItem::Jobs(job_index..job_index + 1),
            ProfileInstant::detailed(self.control.profiler()),
        ));
        self.scheduled_jobs[job_index] = true;
        self.enqueued_job_count += 1;
    }

    /// Marks `file_index` as queued for verification and returns its work item.
    fn start_verification(&mut self, file_index: usize) -> (ReconstructWorkItem, ProfileInstant) {
        if self.verify_started_at.is_none() {
            self.verify_started_at = Some(ProfileInstant::start(self.control.profiler()));
            self.verify_span = self.control.profile_span("installation.reconstruct.verify");
        }
        self.verification_queued[file_index] = true;
        (
            ReconstructWorkItem::VerifyFiles(vec![file_index]),
            ProfileInstant::detailed(self.control.profiler()),
        )
    }

    /// Slows downloads while more than two jobs per worker wait to run.
    fn update_backpressure(&self) {
        let Some(transfer_progress) = self.transfer_progress else {
            return;
        };
        let queue_limit = self.worker_count.saturating_mul(2).max(1);
        let queued_jobs = self
            .enqueued_job_count
            .saturating_sub(self.completed_job_count);
        transfer_progress.set_downstream_backpressure(queued_jobs > queue_limit);
    }

    /// Checks that all work completed and returns the run's timings.
    fn finish(
        mut self,
        pipeline_started_at: ProfileInstant,
    ) -> Result<(ReconstructTimings, VerifyTimings)> {
        if self.completed_job_count != self.jobs.len() {
            return Err(Error::reconstruct(format!(
                "reconstruction completed {} of {} jobs",
                self.completed_job_count,
                self.jobs.len()
            )));
        }
        if self.completed_file_count != self.file_plans.len() {
            return Err(Error::reconstruct(format!(
                "verification completed {} of {} files",
                self.completed_file_count,
                self.file_plans.len()
            )));
        }

        let verify_elapsed = self
            .verify_started_at
            .zip(self.verify_finished_at)
            .map(|(started_at, finished_at)| finished_at.saturating_duration_since(started_at))
            .unwrap_or_default();
        drop(self.verify_span.take());
        self.verify_timings.file_hash += verify_elapsed;
        self.verify_timings.file_hash_bytes = self
            .verify_timings
            .file_hash_bytes
            .saturating_add(self.verified_file_bytes);
        emit_reconstruct_progress(
            self.control,
            "Reconstructing files",
            self.file_plans.len(),
            self.file_plans.len(),
            None,
        );

        self.reconstruct_timings.reconstruct_wall = self
            .reconstruct_finished_at
            .map(|finished_at| finished_at.saturating_duration_since(self.reconstruct_started_at))
            .unwrap_or_default();
        self.reconstruct_timings.total = pipeline_started_at.elapsed();
        self.verify_timings.verify_files_wall = verify_elapsed;
        self.verify_timings.total = verify_elapsed;
        Ok((self.reconstruct_timings, self.verify_timings))
    }
}

/// Counts the jobs that write each output file.
fn count_jobs_per_file(job_file_indices: &[Vec<usize>], file_count: usize) -> Vec<usize> {
    let mut job_counts = vec![0usize; file_count];
    for &file_index in job_file_indices.iter().flatten() {
        if let Some(count) = job_counts.get_mut(file_index) {
            *count += 1;
        }
    }
    job_counts
}

/// Reports reconstruction progress as "`label` (current/total)".
fn emit_reconstruct_progress(
    control: &OperationControl,
    label: &str,
    current: usize,
    total: usize,
    detail: Option<&str>,
) {
    let total = total.max(1);
    emit_progress(
        control.progress(),
        ProgressEvent {
            feedback: None,
            operation: OperationKind::Fetch,
            phase: ProgressPhase::Reconstruct,
            message: format!("{label} ({current}/{total})"),
            current,
            total,
            transferred_bytes: 0,
            total_bytes: 0,
            detail: detail.map(str::to_string),
            done: false,
        },
    );
}

fn validate_artifact_jobs(
    jobs: &[ReconstructJob],
    existing_artifacts: &HashSet<ArtifactRef>,
    pending_downloads: &[PendingArtifactDownload],
) -> Result<HashMap<ArtifactRef, usize>> {
    let pending_artifacts = pending_downloads
        .iter()
        .map(|download| download.artifact.clone())
        .collect::<HashSet<_>>();
    let mut artifact_job_indices = HashMap::new();
    for (job_index, job) in jobs.iter().enumerate() {
        let ReconstructJob::Artifact(job) = job else {
            continue;
        };
        artifact_job_indices.insert(job.artifact.clone(), job_index);
        if !existing_artifacts.contains(&job.artifact) && !pending_artifacts.contains(&job.artifact)
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
    Ok(artifact_job_indices)
}

fn start_artifact_downloads(
    pending_artifact_downloads: &[PendingArtifactDownload],
    artifact_priorities: HashMap<ArtifactRef, ArtifactPriority>,
    transfer_policy: TransferConcurrencyPolicy,
    download_client: Option<&DownloadClient>,
    control: &OperationControl,
    transfer_progress: Option<&Arc<FetchTransferProgress>>,
) -> Result<Option<ArtifactDownloadScheduler>> {
    let scheduler = if pending_artifact_downloads.is_empty() {
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
    Ok(scheduler)
}

fn finish_pipeline_workers(
    artifact_scheduler: Option<ArtifactDownloadScheduler>,
    work_queue: &mut ReconstructWorkQueue,
    event_rx: &mpsc::Receiver<ReconstructJobEvent>,
    control: &OperationControl,
    failed: bool,
) -> Result<()> {
    // Publication can follow return immediately. Keep download shutdown before
    // reconstruction draining, and wait only for work that was dispatched.
    if failed && let Some(scheduler) = artifact_scheduler.as_ref() {
        scheduler.cancel();
    }
    if let Some(scheduler) = artifact_scheduler {
        scheduler.join()?;
    }
    // Queued work has never started and emits no completion event.
    work_queue.ready.clear();
    while failed && work_queue.in_flight > 0 {
        let event = {
            let _wait_span = control.profile_span("installation.coordinator.drain_workers");
            event_rx.recv()
        };
        match event {
            Ok(_) => work_queue.complete(),
            Err(_) => break,
        }
    }
    Ok(())
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
            .targets()
            .map(|target| target.file_index)
            .collect::<Vec<_>>(),
        ReconstructJob::CopySourceFile { file_index, .. } => vec![*file_index],
    };
    file_indices.sort_unstable();
    file_indices.dedup();
    file_indices
}

fn ensure_empty_reconstruct_output_file(
    file_plan: &ReconstructPipelineFile,
    output_path: &Path,
    control: &OperationControl,
) -> Result<()> {
    let _file_span = control
        .profile_span_with_args("installation.reconstruct.prepare.file", || {
            file_trace_args(file_plan.path.clone(), file_plan.size)
        });
    let output = {
        let _create_span = control
            .profile_span_with_args("installation.reconstruct.prepare.file.create", || {
                file_trace_args(file_plan.path.clone(), file_plan.size)
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
                file_trace_args(file_plan.path.clone(), file_plan.size)
            });
        output
            .set_len(file_plan.size)
            .map_err(|e| Error::io_context("sizing empty output file", output_path, e))?;
    }
    Ok(())
}

fn accumulate_pipeline_timings(
    reconstruct_timings: &mut ReconstructTimings,
    verify_timings: &mut VerifyTimings,
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

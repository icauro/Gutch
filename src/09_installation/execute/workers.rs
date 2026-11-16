//! Worker memory admission, job dispatch and terminal job events.

use super::handles::{OutputHandleCache, SourceHandleCache};
use super::job_io::{ReconstructedFileSizeCheck, check_reconstructed_output_size, reconstruct_job};
use super::jobs::{ReconstructJob, ReconstructPipelineFile};
use crate::installation::contracts::ReconstructTimings;
use crate::profiling::ProfileInstant;
use crate::profiling::trace_args;
use crate::progress::OperationControl;
use crate::runtime::MemoryBudget;
use crate::runtime::MemoryPermit;
use crate::{Error, Result};
use rayon::ThreadPool;
use std::collections::VecDeque;
use std::ops::Range;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::{Arc, mpsc};
use std::time::Duration;
// Bound both per-task metadata work and serial hashing before a worker yields.
const MAX_VERIFICATION_BATCH_FILES: usize = 32;
const MAX_VERIFICATION_BATCH_BYTES: u64 = 1024 * 1024;
const MAX_SOURCE_BATCH_JOBS: usize = 32;
const MAX_SOURCE_BATCH_BYTES: u64 = 64 * 1024;

pub(crate) enum ReconstructWorkItem {
    Jobs(Range<usize>),
    VerifyFiles(Vec<usize>),
}

/// Tracks only runnable and dispatched work. Jobs waiting for downloads are
/// owned by the artifact scheduler and enter this queue when ready.
#[derive(Default)]
pub(crate) struct ReconstructWorkQueue {
    pub(crate) ready: VecDeque<(ReconstructWorkItem, ProfileInstant)>,
    // Decremented when the coordinator consumes one terminal event, including failure.
    pub(crate) in_flight: usize,
}

pub(crate) struct AdmittedReconstructWork {
    pub(crate) work: ReconstructWorkItem,
    pub(crate) permit: MemoryPermit,
    pub(crate) queued_at: ProfileInstant,
}

impl ReconstructWorkQueue {
    pub(crate) fn admit_next(
        &mut self,
        jobs: &[ReconstructJob],
        file_plans: &[ReconstructPipelineFile],
        memory: &Arc<MemoryBudget>,
    ) -> Result<Option<AdmittedReconstructWork>> {
        for (index, (work, _)) in self.ready.iter().enumerate() {
            let bytes = match work {
                ReconstructWorkItem::Jobs(indices) => {
                    jobs[indices.start].memory_reservation_bytes()
                }
                ReconstructWorkItem::VerifyFiles(_) => 0,
            };
            if let Some(permit) = memory.try_acquire(bytes)? {
                let (mut work, queued_at) = self.ready.remove(index).expect("admitted queued work");
                // The admitted item was at `index`, so its queue successors now start there.
                match &mut work {
                    ReconstructWorkItem::VerifyFiles(files) => {
                        self.merge_adjacent_verifications(index, files, file_plans);
                    }
                    ReconstructWorkItem::Jobs(indices) => {
                        self.merge_contiguous_source_jobs(index, indices, jobs, bytes);
                    }
                }
                self.in_flight += 1;
                return Ok(Some(AdmittedReconstructWork {
                    work,
                    permit,
                    queued_at,
                }));
            }
        }
        Ok(None)
    }

    /// Merges only adjacent ready verifications, never across a producing job.
    fn merge_adjacent_verifications(
        &mut self,
        next_index: usize,
        files: &mut Vec<usize>,
        file_plans: &[ReconstructPipelineFile],
    ) {
        let total_size =
            |indices: &[usize]| -> u64 { indices.iter().map(|&i| file_plans[i].size).sum() };
        let mut batch_bytes = total_size(files);
        while let Some((ReconstructWorkItem::VerifyFiles(next), _)) = self.ready.get(next_index) {
            let next_bytes = total_size(next);
            if files.len() + next.len() > MAX_VERIFICATION_BATCH_FILES
                || batch_bytes.saturating_add(next_bytes) > MAX_VERIFICATION_BATCH_BYTES
            {
                break;
            }
            files.extend_from_slice(next);
            batch_bytes += next_bytes;
            self.ready.remove(next_index);
        }
    }

    /// Source buffers are used sequentially, so one permit covers the batch. Merges
    /// only consecutive jobs that read one contiguous range of the same source file,
    /// each fitting the first job's permit, without crossing another work kind.
    fn merge_contiguous_source_jobs(
        &mut self,
        next_index: usize,
        indices: &mut Range<usize>,
        jobs: &[ReconstructJob],
        permit_bytes: u64,
    ) {
        let ReconstructJob::SourceChunk(first) = &jobs[indices.start] else {
            return;
        };
        let mut batch_bytes = u64::from(first.raw_size);
        let mut source_end = first.source_offset + batch_bytes;
        while batch_bytes < MAX_SOURCE_BATCH_BYTES && indices.len() < MAX_SOURCE_BATCH_JOBS {
            let Some((ReconstructWorkItem::Jobs(next), _)) = self.ready.get(next_index) else {
                break;
            };
            if next.start != indices.end || next.len() != 1 {
                break;
            }
            let ReconstructJob::SourceChunk(chunk) = &jobs[next.start] else {
                break;
            };
            let chunk_bytes = u64::from(chunk.raw_size);
            if chunk.source_path != first.source_path
                || chunk.source_offset != source_end
                || jobs[next.start].memory_reservation_bytes() > permit_bytes
                || batch_bytes + chunk_bytes > MAX_SOURCE_BATCH_BYTES
            {
                break;
            }
            batch_bytes += chunk_bytes;
            source_end += chunk_bytes;
            indices.end = next.end;
            self.ready.remove(next_index);
        }
    }

    pub(crate) fn complete(&mut self) {
        self.in_flight -= 1;
    }
}

pub(crate) enum ReconstructJobEvent {
    Completed {
        job_indices: Range<usize>,
        timings: Vec<ReconstructTimings>,
    },
    Verified {
        file_count: usize,
        timings: ReconstructedFileSizeCheck,
    },
    Failed(Error),
}

pub(crate) struct ReconstructWorkContext {
    pub(crate) memory: Arc<MemoryBudget>,
    pub(crate) source_handles: SourceHandleCache,
    pub(crate) jobs: Arc<Vec<ReconstructJob>>,
    pub(crate) file_plans: Arc<Vec<ReconstructPipelineFile>>,
    pub(crate) output_paths: Arc<Vec<PathBuf>>,
    pub(crate) output_handles: Arc<OutputHandleCache>,
    pub(crate) control: OperationControl,
    pub(crate) reconstruct_worker_count: usize,
}

pub(crate) fn dispatch_ready_work(
    pool: &ThreadPool,
    work_queue: &mut ReconstructWorkQueue,
    work_context: &Arc<ReconstructWorkContext>,
    event_tx: &mpsc::Sender<ReconstructJobEvent>,
) -> Result<()> {
    // Admit only runnable jobs, leaving pool threads free instead of blocking them
    // on memory. Verification can proceed while larger artifact jobs wait.
    while work_queue.in_flight < work_context.reconstruct_worker_count {
        match work_queue.admit_next(
            &work_context.jobs,
            &work_context.file_plans,
            &work_context.memory,
        ) {
            Ok(Some(admitted)) => {
                spawn_reconstruct_work(
                    pool,
                    admitted.work,
                    work_context,
                    event_tx,
                    admitted.permit,
                    admitted.queued_at,
                );
            }
            Ok(None) => break,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

pub(crate) fn spawn_reconstruct_work(
    pool: &ThreadPool,
    work_item: ReconstructWorkItem,
    context: &Arc<ReconstructWorkContext>,
    event_tx: &mpsc::Sender<ReconstructJobEvent>,
    memory_permit: MemoryPermit,
    queued_at: ProfileInstant,
) {
    let context = Arc::clone(context);
    let event_tx = event_tx.clone();
    pool.spawn(move || {
        let queue_wait = queued_at.elapsed();
        let work_kind = match &work_item {
            ReconstructWorkItem::Jobs(_) => "reconstruct",
            ReconstructWorkItem::VerifyFiles(_) => "verify",
        };
        let _work_span =
            context
                .control
                .profile_span_with_args("installation.worker.execute", || {
                    trace_args!(
                        "queue_wait_us" => queue_wait.as_micros() as u64,
                        "work_kind" => work_kind,
                    )
                });
        let result = catch_unwind(AssertUnwindSafe(|| match work_item {
            ReconstructWorkItem::Jobs(job_indices) => {
                let timings = if job_indices.len() > 1 {
                    super::job_io::reconstruct_source_chunk_batch(
                        &context.jobs[job_indices.clone()],
                        &context.output_handles,
                        &context.source_handles,
                        &context.control,
                    )?
                } else {
                    let mut timings = Vec::with_capacity(job_indices.len());
                    for job_index in job_indices.clone() {
                        timings.push(reconstruct_job(super::job_io::ReconstructJobRequest {
                            job: &context.jobs[job_index],
                            file_plans: &context.file_plans,
                            output_paths: &context.output_paths,
                            output_handles: &context.output_handles,
                            source_handles: &context.source_handles,
                            control: &context.control,
                        })?);
                    }
                    timings
                };
                Ok(ReconstructJobEvent::Completed {
                    job_indices,
                    timings,
                })
            }
            ReconstructWorkItem::VerifyFiles(file_indices) => {
                let mut timings = ReconstructedFileSizeCheck {
                    elapsed: Duration::ZERO,
                    bytes: 0,
                };
                for &file_index in &file_indices {
                    // Every job writing this file has finished.
                    context.output_handles.release(file_index)?;
                    let file = check_reconstructed_output_size(
                        &context.file_plans[file_index],
                        &context.output_paths[file_index],
                        &context.control,
                    )?;
                    timings.elapsed += file.elapsed;
                    timings.bytes = timings.bytes.saturating_add(file.bytes);
                }
                Ok(ReconstructJobEvent::Verified {
                    file_count: file_indices.len(),
                    timings,
                })
            }
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

#[cfg(test)]
#[path = "workers_tests.rs"]
mod tests;

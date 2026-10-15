use super::handles::{OutputHandleCache, SourceHandleCache};
use super::io::{ReconstructedFileVerification, reconstruct_job, verify_reconstructed_output_file};
use super::jobs::{ReconstructJob, ReconstructPipelineFile};
use super::types::ReconstructTimings;
use crate::profile::ProfileInstant;
use crate::progress::GutchOperationControl;
use crate::{Error, Result};
use rayon::ThreadPool;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
pub(super) enum ReconstructWorkItem {
    Job(usize),
    VerifyFile(usize),
}

/// Tracks only runnable and dispatched work. Jobs waiting for downloads are
/// owned by the artifact scheduler and enter this queue when ready.
#[derive(Default)]
pub(super) struct ReconstructionWorkQueue {
    pub(super) ready: std::collections::VecDeque<(ReconstructWorkItem, ProfileInstant)>,
    // Decremented when the coordinator consumes one terminal event, including failure.
    pub(super) in_flight: usize,
}

pub(super) struct AdmittedReconstructionWork {
    pub(super) work: ReconstructWorkItem,
    pub(super) permit: crate::runtime::MemoryPermit,
    pub(super) queued_at: ProfileInstant,
}

impl ReconstructionWorkQueue {
    pub(super) fn admit_next(
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

    pub(super) fn complete(&mut self) {
        self.in_flight -= 1;
    }
}

pub(super) enum ReconstructJobEvent {
    Completed {
        job_index: usize,
        timings: Box<ReconstructTimings>,
    },
    Verified(ReconstructedFileVerification),
    Failed(Error),
}

pub(super) struct ReconstructWorkContext {
    pub(super) memory: Arc<crate::runtime::MemoryBudget>,
    pub(super) source_handles: SourceHandleCache,
    pub(super) jobs: Arc<Vec<ReconstructJob>>,
    pub(super) file_plans: Arc<Vec<ReconstructPipelineFile>>,
    pub(super) output_paths: Arc<Vec<PathBuf>>,
    pub(super) precomputed_file_hashes: Arc<Vec<Mutex<Option<crate::artifact::hash::ChunkHash>>>>,
    pub(super) output_handles: Arc<OutputHandleCache>,
    pub(super) compression_profile: Arc<crate::artifact::profiles::CompressionProfile>,
    pub(super) control: GutchOperationControl,
    pub(super) reconstruct_worker_count: usize,
}

pub(super) fn dispatch_ready_work(
    pool: &ThreadPool,
    work_queue: &mut ReconstructionWorkQueue,
    work_context: &Arc<ReconstructWorkContext>,
    event_tx: &mpsc::Sender<ReconstructJobEvent>,
) -> Result<()> {
    // Admit only runnable jobs, leaving pool threads free instead of blocking them
    // on memory. Verification can proceed while larger artifact jobs wait.
    while work_queue.in_flight < work_context.reconstruct_worker_count {
        match work_queue.admit_next(&work_context.jobs, &work_context.memory) {
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

pub(super) fn spawn_reconstruct_work(
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn work_queue_verifies_while_source_job_waits_for_memory_and_drains_only_dispatched_work() {
        let memory = Arc::new(crate::runtime::MemoryBudget::new(2 * 1024 * 1024));
        let occupied = memory.try_acquire(2 * 1024 * 1024).unwrap().unwrap();
        let jobs = vec![ReconstructJob::CopySourceFile {
            file_index: 0,
            source_path: "unused".into(),
        }];
        let mut queue = ReconstructionWorkQueue::default();
        queue
            .ready
            .push_back((ReconstructWorkItem::Job(0), ProfileInstant::detailed(None)));
        queue.ready.push_back((
            ReconstructWorkItem::VerifyFile(1),
            ProfileInstant::detailed(None),
        ));
        let verification = queue.admit_next(&jobs, &memory).unwrap().unwrap();
        assert!(matches!(
            verification.work,
            ReconstructWorkItem::VerifyFile(1)
        ));
        assert_eq!(queue.in_flight, 1);
        assert!(queue.admit_next(&jobs, &memory).unwrap().is_none());
        drop(occupied);
        let source = queue.admit_next(&jobs, &memory).unwrap().unwrap();
        assert!(matches!(source.work, ReconstructWorkItem::Job(0)));
        assert_eq!(queue.in_flight, 2);
        queue.ready.push_back((
            ReconstructWorkItem::VerifyFile(2),
            ProfileInstant::detailed(None),
        ));
        queue.ready.clear();
        assert_eq!(
            queue.in_flight, 2,
            "discarding queued work must not hide dispatched work"
        );
        drop(verification);
        queue.complete();
        drop(source);
        queue.complete();
        assert_eq!(queue.in_flight, 0);
        assert!(memory.try_acquire(2 * 1024 * 1024).unwrap().is_some());
    }
}

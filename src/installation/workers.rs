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
// Bound both per-task metadata work and serial hashing before a worker yields.
const MAX_VERIFICATION_BATCH_FILES: usize = 32;
const MAX_VERIFICATION_BATCH_BYTES: u64 = 1024 * 1024;

pub(super) enum ReconstructWorkItem {
    Job(usize),
    VerifyFiles(Vec<usize>),
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
        file_plans: &[ReconstructPipelineFile],
        memory: &Arc<crate::runtime::MemoryBudget>,
    ) -> Result<Option<AdmittedReconstructionWork>> {
        for (index, (work, _)) in self.ready.iter().enumerate() {
            let bytes = match work {
                ReconstructWorkItem::Job(job_index) => jobs[*job_index].memory_reservation_bytes(),
                ReconstructWorkItem::VerifyFiles(_) => 0,
            };
            if let Some(permit) = memory.try_acquire(bytes)? {
                let (mut work, queued_at) = self.ready.remove(index).expect("admitted queued work");
                // Merge only adjacent ready verifications, never across a producing job.
                if let ReconstructWorkItem::VerifyFiles(files) = &mut work {
                    let mut bytes: u64 = files.iter().map(|&i| file_plans[i].size).sum();
                    while let Some((ReconstructWorkItem::VerifyFiles(next), _)) =
                        self.ready.get(index)
                    {
                        let next_bytes: u64 = next.iter().map(|&i| file_plans[i].size).sum();
                        if files.len() + next.len() > MAX_VERIFICATION_BATCH_FILES
                            || bytes.saturating_add(next_bytes) > MAX_VERIFICATION_BATCH_BYTES
                        {
                            break;
                        }
                        let (ReconstructWorkItem::VerifyFiles(next), _) =
                            self.ready.remove(index).unwrap()
                        else {
                            unreachable!("next queued work is verification");
                        };
                        files.extend(next);
                        bytes += next_bytes;
                    }
                }
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
    Verified {
        file_count: usize,
        timings: ReconstructedFileVerification,
    },
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
                                ReconstructWorkItem::VerifyFiles(_) => "verify",
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
            ReconstructWorkItem::VerifyFiles(file_indices) => {
                let mut timings = ReconstructedFileVerification {
                    elapsed: std::time::Duration::ZERO,
                    bytes: 0,
                };
                for &file_index in &file_indices {
                    let file = verify_reconstructed_output_file(
                        file_index,
                        &context.file_plans[file_index],
                        &context.output_paths,
                        &context.precomputed_file_hashes,
                        &context.control,
                        context.reconstruct_worker_count,
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
        let files = (0..3)
            .map(|i| ReconstructPipelineFile {
                path: i.to_string(),
                size: 4,
                full_file_hash: crate::artifact::hash::ChunkHash::from_blake3(blake3::hash(
                    b"data",
                )),
            })
            .collect::<Vec<_>>();
        let mut queue = ReconstructionWorkQueue::default();
        queue
            .ready
            .push_back((ReconstructWorkItem::Job(0), ProfileInstant::detailed(None)));
        queue.ready.push_back((
            ReconstructWorkItem::VerifyFiles(vec![1]),
            ProfileInstant::detailed(None),
        ));
        let verification = queue.admit_next(&jobs, &files, &memory).unwrap().unwrap();
        assert!(matches!(
            verification.work,
            ReconstructWorkItem::VerifyFiles(ref files) if files == &[1]
        ));
        assert_eq!(queue.in_flight, 1);
        assert!(queue.admit_next(&jobs, &files, &memory).unwrap().is_none());
        drop(occupied);
        let source = queue.admit_next(&jobs, &files, &memory).unwrap().unwrap();
        assert!(matches!(source.work, ReconstructWorkItem::Job(0)));
        assert_eq!(queue.in_flight, 2);
        queue.ready.push_back((
            ReconstructWorkItem::VerifyFiles(vec![2]),
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

    fn verification_files(sizes: &[u64]) -> Vec<ReconstructPipelineFile> {
        sizes
            .iter()
            .enumerate()
            .map(|(index, &size)| ReconstructPipelineFile {
                path: index.to_string(),
                size,
                full_file_hash: crate::artifact::hash::ChunkHash::from_blake3(blake3::hash(
                    b"data",
                )),
            })
            .collect()
    }

    #[test]
    fn verification_batches_bound_file_count_bytes_and_preserve_job_boundaries() {
        let memory = Arc::new(crate::runtime::MemoryBudget::new(1024 * 1024));
        for (sizes, expected_lengths) in [
            (vec![1; 65], vec![32, 32, 1]),
            (vec![600 * 1024, 600 * 1024, 1], vec![1, 2]),
            (vec![2 * 1024 * 1024, 2 * 1024 * 1024, 1], vec![1, 1, 1]),
        ] {
            let files = verification_files(&sizes);
            let mut queue = ReconstructionWorkQueue::default();
            for index in 0..files.len() {
                queue.ready.push_back((
                    ReconstructWorkItem::VerifyFiles(vec![index]),
                    ProfileInstant::detailed(None),
                ));
            }
            let mut actual_order = Vec::new();
            for expected_length in expected_lengths {
                let admitted = queue.admit_next(&[], &files, &memory).unwrap().unwrap();
                let ReconstructWorkItem::VerifyFiles(indices) = &admitted.work else {
                    panic!("expected verification")
                };
                assert_eq!(indices.len(), expected_length);
                actual_order.extend(indices.iter().copied());
                assert_eq!(queue.in_flight, 1);
                drop(admitted);
                queue.complete();
            }
            assert_eq!(actual_order, (0..files.len()).collect::<Vec<_>>());
            assert!(queue.ready.is_empty());
            assert_eq!(queue.in_flight, 0);
        }
        let files = verification_files(&[1, 1]);
        let jobs = [ReconstructJob::CopySourceFile {
            file_index: 0,
            source_path: "unused".into(),
        }];
        let mut queue = ReconstructionWorkQueue::default();
        for work in [
            ReconstructWorkItem::VerifyFiles(vec![0]),
            ReconstructWorkItem::Job(0),
            ReconstructWorkItem::VerifyFiles(vec![1]),
        ] {
            queue
                .ready
                .push_back((work, ProfileInstant::detailed(None)));
        }
        let admitted = queue.admit_next(&jobs, &files, &memory).unwrap().unwrap();
        assert!(
            matches!(admitted.work, ReconstructWorkItem::VerifyFiles(ref indices) if indices == &[0])
        );
        assert!(matches!(
            queue.ready.front().unwrap().0,
            ReconstructWorkItem::Job(0)
        ));
    }

    #[test]
    fn failed_or_cancelled_verification_batch_releases_resources_before_terminal_event() {
        #[derive(Debug)]
        struct CancelAfterFirstFile(std::sync::atomic::AtomicUsize);
        impl crate::progress::GutchCancellationHook for CancelAfterFirstFile {
            fn is_cancelled(&self) -> bool {
                // A forwarded file checks twice; cancellation begins at the next file.
                self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= 2
            }
        }
        for cancel in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let files = verification_files(&[4, 4, 4]);
            let paths = Arc::new(
                files
                    .iter()
                    .map(|file| directory.path().join(&file.path))
                    .collect::<Vec<_>>(),
            );
            std::fs::write(&paths[0], b"data").unwrap();
            std::fs::write(&paths[1], b"bad!").unwrap();
            // The last file is absent: the first hash mismatch must stop the batch.
            let memory = Arc::new(crate::runtime::MemoryBudget::new(4096));
            let permit = memory.try_acquire(4096).unwrap().unwrap();
            let handles = Arc::new(OutputHandleCache::new(
                paths.clone(),
                Arc::new(vec![4; 3]),
                &[1; 3],
                1,
            ));
            let weak_handles = Arc::downgrade(&handles);
            let mut control = GutchOperationControl::default();
            if cancel {
                control = control.with_cancellation(Arc::new(CancelAfterFirstFile(
                    std::sync::atomic::AtomicUsize::new(0),
                )));
            }
            let first_hash = files[0].full_file_hash;
            let context = Arc::new(ReconstructWorkContext {
                memory: memory.clone(),
                source_handles: SourceHandleCache::default(),
                jobs: Arc::new(Vec::new()),
                file_plans: Arc::new(files),
                output_paths: paths,
                precomputed_file_hashes: Arc::new(
                    (0..3)
                        .map(|index| Mutex::new((cancel && index == 0).then_some(first_hash)))
                        .collect(),
                ),
                output_handles: handles,
                compression_profile: Arc::new(
                    crate::artifact::profiles::CompressionProfile::default(),
                ),
                control,
                reconstruct_worker_count: 1,
            });
            let weak_context = Arc::downgrade(&context);
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(1)
                .build()
                .unwrap();
            let (sender, receiver) = mpsc::channel();
            spawn_reconstruct_work(
                &pool,
                ReconstructWorkItem::VerifyFiles(vec![0, 1, 2]),
                &context,
                &sender,
                permit,
                ProfileInstant::detailed(None),
            );
            drop(context);
            let event = receiver
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            if cancel {
                assert!(matches!(
                    event,
                    ReconstructJobEvent::Failed(Error::Cancelled)
                ));
            } else {
                assert!(matches!(
                    event,
                    ReconstructJobEvent::Failed(Error::HashMismatch { .. })
                ));
            }
            assert!(weak_context.upgrade().is_none());
            assert!(weak_handles.upgrade().is_none());
            assert!(memory.try_acquire(4096).unwrap().is_some());
            assert!(
                receiver.try_recv().is_err(),
                "a batch emits one terminal event"
            );
        }
    }
}

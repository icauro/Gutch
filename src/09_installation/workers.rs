use super::handles::{OutputHandleCache, SourceHandleCache};
use super::io::{ReconstructedFileVerification, reconstruct_job, verify_reconstructed_output_file};
use super::jobs::{ReconstructJob, ReconstructPipelineFile};
use super::types::ReconstructTimings;
use crate::profiling::ProfileInstant;
use crate::progress::OperationControl;
use crate::{Error, Result};
use rayon::ThreadPool;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::{Arc, mpsc};
// Bound both per-task metadata work and serial hashing before a worker yields.
const MAX_VERIFICATION_BATCH_FILES: usize = 32;
const MAX_VERIFICATION_BATCH_BYTES: u64 = 1024 * 1024;
const MAX_SOURCE_BATCH_JOBS: usize = 32;
const MAX_SOURCE_BATCH_BYTES: u64 = 64 * 1024;

pub(super) enum ReconstructWorkItem {
    Jobs(std::ops::Range<usize>),
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
                return Ok(Some(AdmittedReconstructionWork {
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
        indices: &mut std::ops::Range<usize>,
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

    pub(super) fn complete(&mut self) {
        self.in_flight -= 1;
    }
}

pub(super) enum ReconstructJobEvent {
    Completed {
        job_indices: std::ops::Range<usize>,
        timings: Vec<ReconstructTimings>,
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
    pub(super) output_handles: Arc<OutputHandleCache>,
    pub(super) compression_profile: Arc<crate::artifact::profiles::CompressionProfile>,
    pub(super) control: OperationControl,
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
                                ReconstructWorkItem::Jobs(_) => "reconstruct",
                                ReconstructWorkItem::VerifyFiles(_) => "verify",
                            }),
                        ),
                    ])
                });
        let result = catch_unwind(AssertUnwindSafe(|| match work_item {
            ReconstructWorkItem::Jobs(job_indices) => {
                let timings = if job_indices.len() > 1 {
                    super::io::reconstruct_source_chunk_batch(
                        &context.jobs[job_indices.clone()],
                        &context.output_handles,
                        &context.source_handles,
                        &context.control,
                    )?
                } else {
                    let mut timings = Vec::with_capacity(job_indices.len());
                    for job_index in job_indices.clone() {
                        timings.push(reconstruct_job(super::io::ReconstructJobRequest {
                            job: &context.jobs[job_index],
                            file_plans: &context.file_plans,
                            output_paths: &context.output_paths,
                            output_handles: &context.output_handles,
                            source_handles: &context.source_handles,
                            compression_profile: &context.compression_profile,
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
                let mut timings = ReconstructedFileVerification {
                    elapsed: std::time::Duration::ZERO,
                    bytes: 0,
                };
                for &file_index in &file_indices {
                    let file = verify_reconstructed_output_file(
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
                full_file_hash: crate::artifact::hash::ChunkHash::of(b"data"),
            })
            .collect::<Vec<_>>();
        let mut queue = ReconstructionWorkQueue::default();
        queue.ready.push_back((
            ReconstructWorkItem::Jobs(0..1),
            ProfileInstant::detailed(None),
        ));
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
        assert!(
            matches!(source.work, ReconstructWorkItem::Jobs(ref indices) if indices == &(0..1))
        );
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
                full_file_hash: crate::artifact::hash::ChunkHash::of(b"data"),
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
            ReconstructWorkItem::Jobs(0..1),
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
            ReconstructWorkItem::Jobs(ref indices) if indices == &(0..1)
        ));
    }

    #[test]
    fn failed_or_cancelled_verification_batch_releases_resources_before_terminal_event() {
        #[derive(Debug)]
        struct CancelAfterFirstFile(std::sync::atomic::AtomicUsize);
        impl crate::progress::CancellationHook for CancelAfterFirstFile {
            fn is_cancelled(&self) -> bool {
                // Each file checks once; cancellation begins at the second file.
                self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= 1
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
            std::fs::write(&paths[1], b"bad").unwrap();
            // The last file is absent: the first size mismatch must stop the batch.
            let memory = Arc::new(crate::runtime::MemoryBudget::new(4096));
            let permit = memory.try_acquire(4096).unwrap().unwrap();
            let handles = Arc::new(OutputHandleCache::new(
                paths.clone(),
                Arc::new(vec![4; 3]),
                &[1; 3],
                1,
            ));
            let weak_handles = Arc::downgrade(&handles);
            let mut control = OperationControl::default();
            if cancel {
                control = control.with_cancellation(Arc::new(CancelAfterFirstFile(
                    std::sync::atomic::AtomicUsize::new(0),
                )));
            }
            let context = Arc::new(ReconstructWorkContext {
                memory: memory.clone(),
                source_handles: SourceHandleCache::default(),
                jobs: Arc::new(Vec::new()),
                file_plans: Arc::new(files),
                output_paths: paths,
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
                    ReconstructJobEvent::Failed(Error::Reconstruct(_))
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

    fn source_job(path: &Arc<PathBuf>, offset: u64, size: u32) -> ReconstructJob {
        ReconstructJob::SourceChunk(super::super::jobs::ReconstructSourceChunkJob {
            chunk_hash: crate::artifact::hash::ChunkHash::of(b"data"),
            raw_size: size,
            source_path: Arc::clone(path),
            source_offset: offset,
            first_target: super::super::jobs::ReconstructTarget {
                file_index: 0,
                file_offset: offset,
            },
            additional_targets: Vec::new(),
        })
    }

    #[test]
    fn source_batches_bound_jobs_bytes_and_reservations() {
        for (sizes, expected) in [
            (vec![4; 65], vec![32, 32, 1]),
            (vec![40 * 1024, 40 * 1024, 1], vec![1, 2]),
            (vec![128 * 1024, 4], vec![1, 1]),
            (vec![4, 8, 4], vec![1, 2]),
        ] {
            let path = Arc::new(PathBuf::from("source"));
            let mut offset = 0;
            let jobs = sizes
                .iter()
                .map(|&size| {
                    let job = source_job(&path, offset, size);
                    offset += u64::from(size);
                    job
                })
                .collect::<Vec<_>>();
            let memory = Arc::new(crate::runtime::MemoryBudget::new(2 * 1024 * 1024));
            let mut queue = ReconstructionWorkQueue::default();
            for index in 0..jobs.len() {
                queue.ready.push_back((
                    ReconstructWorkItem::Jobs(index..index + 1),
                    ProfileInstant::detailed(None),
                ));
            }
            let mut start = 0;
            for length in expected {
                let admitted = queue.admit_next(&jobs, &[], &memory).unwrap().unwrap();
                assert!(
                    matches!(&admitted.work, ReconstructWorkItem::Jobs(indices) if indices == &(start..start + length))
                );
                assert_eq!(queue.in_flight, 1);
                let reserved = jobs[start].memory_reservation_bytes();
                // Sequential jobs share one permit, not one permit per chunk.
                assert!(
                    memory
                        .try_acquire(2 * 1024 * 1024 - reserved + 1)
                        .unwrap()
                        .is_none()
                );
                assert!(
                    memory
                        .try_acquire(2 * 1024 * 1024 - reserved)
                        .unwrap()
                        .is_some()
                );
                drop(admitted);
                queue.complete();
                start += length;
            }
            assert_eq!(start, jobs.len());
            assert!(queue.ready.is_empty());
            assert!(memory.try_acquire(2 * 1024 * 1024).unwrap().is_some());
        }
    }

    #[test]
    fn source_batches_preserve_source_and_work_boundaries() {
        let first = Arc::new(PathBuf::from("first"));
        let second = Arc::new(PathBuf::from("second"));
        let jobs = vec![
            source_job(&first, 0, 4),
            source_job(&second, 0, 4),
            source_job(&second, 4, 4),
            ReconstructJob::CopySourceFile {
                file_index: 0,
                source_path: PathBuf::from("copied"),
            },
            source_job(&second, 8, 4),
        ];
        let files = verification_files(&[4]);
        let memory = Arc::new(crate::runtime::MemoryBudget::new(2 * 1024 * 1024));
        let mut queue = ReconstructionWorkQueue::default();
        for work in [
            ReconstructWorkItem::Jobs(0..1),
            ReconstructWorkItem::Jobs(1..2),
            ReconstructWorkItem::VerifyFiles(vec![0]),
            ReconstructWorkItem::Jobs(2..3),
            ReconstructWorkItem::Jobs(3..4),
            ReconstructWorkItem::Jobs(4..5),
        ] {
            queue
                .ready
                .push_back((work, ProfileInstant::detailed(None)));
        }
        for index in 0..jobs.len() {
            let admitted = queue.admit_next(&jobs, &files, &memory).unwrap().unwrap();
            assert!(
                matches!(&admitted.work, ReconstructWorkItem::Jobs(indices) if indices == &(index..index+1))
            );
            drop(admitted);
            queue.complete();
            if index == 1 {
                let verification = queue.admit_next(&jobs, &files, &memory).unwrap().unwrap();
                assert!(matches!(
                    verification.work,
                    ReconstructWorkItem::VerifyFiles(_)
                ));
                drop(verification);
                queue.complete();
            }
        }
        assert!(queue.ready.is_empty());

        // A gap in the source range ends the batch even for consecutive jobs.
        let jobs = vec![
            source_job(&first, 0, 4),
            source_job(&first, 4, 4),
            source_job(&first, 16, 4),
        ];
        for index in 0..jobs.len() {
            queue.ready.push_back((
                ReconstructWorkItem::Jobs(index..index + 1),
                ProfileInstant::detailed(None),
            ));
        }
        let admitted = queue.admit_next(&jobs, &files, &memory).unwrap().unwrap();
        assert!(matches!(&admitted.work, ReconstructWorkItem::Jobs(indices) if indices == &(0..2)));
        drop(admitted);
        queue.complete();
        queue.ready.clear();

        // An unqueued job between ready indexes must not enter the batch.
        let jobs = (0..3)
            .map(|index| source_job(&first, index * 4, 4))
            .collect::<Vec<_>>();
        for index in [0, 2] {
            queue.ready.push_back((
                ReconstructWorkItem::Jobs(index..index + 1),
                ProfileInstant::detailed(None),
            ));
        }
        let admitted = queue.admit_next(&jobs, &files, &memory).unwrap().unwrap();
        assert!(matches!(&admitted.work, ReconstructWorkItem::Jobs(indices) if indices == &(0..1)));
        assert!(
            matches!(&queue.ready.front().unwrap().0, ReconstructWorkItem::Jobs(indices) if indices == &(2..3))
        );
    }

    #[test]
    fn source_batch_reports_success_or_stops_on_failure_and_releases_resources() {
        #[derive(Debug)]
        struct CancelAfterFirstChunk(std::sync::atomic::AtomicUsize);
        impl crate::progress::CancellationHook for CancelAfterFirstChunk {
            fn is_cancelled(&self) -> bool {
                self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= 2
            }
        }
        for outcome in [
            "success",
            "short_read",
            "hash_failure",
            "cancel",
            "flush_failure",
            "fanout",
        ] {
            let cancel = outcome == "cancel";
            let root = tempfile::tempdir().unwrap();
            let source = Arc::new(root.path().join("source"));
            let source_bytes: &[u8] = match outcome {
                "success" | "flush_failure" | "fanout" => b"datadatadata",
                "short_read" => b"data",
                _ => b"databad!data",
            };
            std::fs::write(source.as_ref(), source_bytes).unwrap();
            let jobs = (0..3)
                .map(|index| {
                    let mut job = source_job(&source, index * 4, 4);
                    if outcome == "fanout" {
                        let ReconstructJob::SourceChunk(chunk) = &mut job else {
                            unreachable!()
                        };
                        chunk
                            .additional_targets
                            .push(super::super::jobs::ReconstructTarget {
                                file_index: 1,
                                file_offset: index * 4,
                            });
                    }
                    job
                })
                .collect::<Vec<_>>();
            let file_count = if outcome == "fanout" { 2 } else { 1 };
            let files = verification_files(&vec![12; file_count]);
            let output = if outcome == "flush_failure" {
                root.path().join("missing/output")
            } else {
                root.path().join("output")
            };
            let second_output = root.path().join("second_output");
            let paths = Arc::new(if outcome == "fanout" {
                vec![output.clone(), second_output.clone()]
            } else {
                vec![output.clone()]
            });
            let memory = Arc::new(crate::runtime::MemoryBudget::new(2 * 1024 * 1024));
            let permit = memory.try_acquire(2 * 1024 * 1024).unwrap().unwrap();
            let handles = Arc::new(OutputHandleCache::new(
                Arc::clone(&paths),
                Arc::new(vec![12; file_count]),
                &vec![3; file_count],
                1,
            ));
            let weak_handles = Arc::downgrade(&handles);
            let control = if cancel {
                OperationControl::default().with_cancellation(Arc::new(CancelAfterFirstChunk(
                    std::sync::atomic::AtomicUsize::new(0),
                )))
            } else {
                OperationControl::default()
            };
            let context = Arc::new(ReconstructWorkContext {
                memory: Arc::clone(&memory),
                source_handles: SourceHandleCache::default(),
                jobs: Arc::new(jobs),
                file_plans: Arc::new(files),
                output_paths: paths,
                output_handles: handles,
                compression_profile: Arc::new(Default::default()),
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
                ReconstructWorkItem::Jobs(0..3),
                &context,
                &sender,
                permit,
                ProfileInstant::detailed(None),
            );
            drop(context);
            let event = receiver
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            match outcome {
                "success" | "fanout" => {
                    let ReconstructJobEvent::Completed {
                        job_indices,
                        timings,
                    } = event
                    else {
                        panic!("expected completed source batch")
                    };
                    assert_eq!(job_indices, 0..3);
                    assert_eq!(timings.len(), 3);
                    assert_eq!(
                        timings
                            .iter()
                            .map(|job| job.chunk_verify_bytes)
                            .sum::<u64>(),
                        12
                    );
                    assert_eq!(
                        timings.iter().map(|job| job.file_write_bytes).sum::<u64>(),
                        if outcome == "fanout" { 24 } else { 12 }
                    );
                }
                "short_read" => {
                    let ReconstructJobEvent::Failed(Error::IoContext { source, .. }) = event else {
                        panic!("expected source read failure")
                    };
                    assert_eq!(source.kind(), std::io::ErrorKind::UnexpectedEof);
                }
                "flush_failure" => {
                    let ReconstructJobEvent::Failed(Error::IoContext { source, .. }) = event else {
                        panic!("expected final flush failure")
                    };
                    assert_eq!(source.kind(), std::io::ErrorKind::NotFound);
                    std::fs::create_dir_all(output.parent().unwrap()).unwrap();
                }
                "cancel" => assert!(matches!(
                    event,
                    ReconstructJobEvent::Failed(Error::Cancelled)
                )),
                _ => assert!(matches!(
                    event,
                    ReconstructJobEvent::Failed(Error::HashMismatch { .. })
                )),
            }
            assert!(weak_context.upgrade().is_none());
            assert!(weak_handles.upgrade().is_none());
            assert!(memory.try_acquire(2 * 1024 * 1024).unwrap().is_some());
            if matches!(outcome, "success" | "fanout") {
                assert_eq!(std::fs::read(&output).unwrap(), b"datadatadata");
                if outcome == "fanout" {
                    assert_eq!(std::fs::read(&second_output).unwrap(), b"datadatadata");
                }
            } else {
                assert!(
                    !output.exists(),
                    "failed source batch must discard its pending range"
                );
            }
            assert!(
                receiver.try_recv().is_err(),
                "a batch emits one terminal event"
            );
        }
    }
}

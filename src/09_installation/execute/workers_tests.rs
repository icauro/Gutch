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
    let mut queue = ReconstructWorkQueue::default();
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
    assert!(matches!(source.work, ReconstructWorkItem::Jobs(ref indices) if indices == &(0..1)));
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
        let mut queue = ReconstructWorkQueue::default();
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
    let mut queue = ReconstructWorkQueue::default();
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
            // Each file checks once; cancellation begins at the second file.
            control = control.with_cancellation(Arc::new(
                crate::test_util::TestCancellation::after_checks(1),
            ));
        }
        let context = Arc::new(ReconstructWorkContext {
            memory: memory.clone(),
            source_handles: SourceHandleCache::default(),
            jobs: Arc::new(Vec::new()),
            file_plans: Arc::new(files),
            output_paths: paths,
            output_handles: handles,
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
        let mut queue = ReconstructWorkQueue::default();
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
    let mut queue = ReconstructWorkQueue::default();
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
            OperationControl::default().with_cancellation(Arc::new(
                crate::test_util::TestCancellation::after_checks(2),
            ))
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

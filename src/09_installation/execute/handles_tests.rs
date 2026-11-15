use super::*;
use std::io::Write;

#[test]
fn buffered_output_preserves_contiguous_ranges_gaps_switches_and_backward_writes() {
    let directory = tempfile::tempdir().unwrap();
    let paths = Arc::new(vec![
        directory.path().join("first"),
        directory.path().join("second"),
    ]);
    let cache = OutputHandleCache::new(paths.clone(), Arc::new(vec![16, 8]), &[1, 1], 1);
    let control = OperationControl::default();
    let mut writer = ReconstructOutputWriter::new(&cache, &control);
    writer.write(0, 0, b"ab").unwrap();
    writer.write(0, 2, b"cd").unwrap();
    assert!(!paths[0].exists());
    writer.write(0, 8, b"gap").unwrap();
    writer.write(0, 1, b"X").unwrap();
    writer.write_unbuffered(1, 2, b"other").unwrap();
    writer.write(0, 2, b"YZ").unwrap();
    writer.flush().unwrap();
    writer.flush().unwrap();
    let mut first = vec![0; 16];
    first[..4].copy_from_slice(b"aXYZ");
    first[8..11].copy_from_slice(b"gap");
    let mut second = vec![0; 8];
    second[2..7].copy_from_slice(b"other");
    assert_eq!(std::fs::read(&paths[0]).unwrap(), first);
    assert_eq!(std::fs::read(&paths[1]).unwrap(), second);
}

#[test]
fn buffered_output_bounds_capacity_and_bypasses_large_ranges() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("output");
    let cache = OutputHandleCache::new(
        Arc::new(vec![path.clone()]),
        Arc::new(vec![RECONSTRUCT_OUTPUT_BUFFER_BYTES as u64 + 2]),
        &[1],
        1,
    );
    let control = OperationControl::default();
    let mut writer = ReconstructOutputWriter::new(&cache, &control);
    let large = vec![7; RECONSTRUCT_OUTPUT_BUFFER_BYTES + 1];
    writer.write(0, 0, &large).unwrap();
    assert_eq!(writer.bytes.capacity(), 0);
    let prefix = vec![1; RECONSTRUCT_OUTPUT_BUFFER_BYTES - 1];
    writer.write(0, 0, &prefix).unwrap();
    let allocated_capacity = writer.bytes.capacity();
    writer.write(0, prefix.len() as u64, &[2, 3]).unwrap();
    assert_eq!(writer.bytes.len(), 2);
    assert_eq!(writer.bytes.capacity(), allocated_capacity);
    writer.flush().unwrap();
    let mut expected = prefix;
    expected.extend_from_slice(&[2, 3, 0]);
    assert_eq!(std::fs::read(path).unwrap(), expected);
}

#[test]
fn buffered_output_propagates_flush_errors_and_discards_pending_bytes_on_drop() {
    for cancel in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("missing-parent/output");
        let cache =
            OutputHandleCache::new(Arc::new(vec![path.clone()]), Arc::new(vec![8]), &[1], 1);
        let cancellation = Arc::new(crate::test_util::TestCancellation::default());
        let control = OperationControl::default().with_cancellation(cancellation.clone());
        let mut writer = ReconstructOutputWriter::new(&cache, &control);
        writer.write(0, 0, b"data").unwrap();
        if cancel {
            cancellation.cancel();
        }
        let error = writer.flush().unwrap_err();
        if cancel {
            assert!(matches!(error, Error::Cancelled));
        } else {
            assert!(matches!(error, Error::IoContext { .. }));
        }
        // A later successful open must not cause drop to retry a failed flush.
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        drop(writer);
        assert!(!path.exists());
    }
}

#[test]
fn output_open_failure_identifies_operation_path_and_native_source() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("missing-parent/output");
    let cache = OutputHandleCache::new(Arc::new(vec![path.clone()]), Arc::new(vec![8]), &[1], 1);
    let error = cache.get_or_open(0).unwrap_err();
    assert!(error.to_string().contains("opening reconstruction output"));
    assert!(error.to_string().contains(&path.display().to_string()));
    let Error::IoContext { source, .. } = error else {
        panic!("missing I/O context")
    };
    assert_eq!(source.kind(), std::io::ErrorKind::NotFound);
}

#[test]
fn source_ranges_handle_seeks_partial_reads_and_concurrency() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("source");
    std::fs::write(&path, b"abcdefghijkl").unwrap();
    let cache = SourceHandleCache::default();
    let mut bytes = [0; 4];
    for (offset, expected) in [(0, b"abcd"), (4, b"efgh"), (0, b"abcd"), (8, b"ijkl")] {
        cache.read_range(&path, offset, &mut bytes).unwrap();
        assert_eq!(&bytes, expected);
    }
    assert!(cache.read_range(&path, 10, &mut bytes).is_err());
    // The failed read advanced to EOF. Extend the same file, then retry at
    // its original offset to ensure the stale position is never reused.
    OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"mnop")
        .unwrap();
    cache.read_range(&path, 10, &mut bytes).unwrap();
    assert_eq!(&bytes, b"klmn");
    std::thread::scope(|scope| {
        for (offset, expected) in [(0, b"abcd"), (4, b"efgh"), (8, b"ijkl")] {
            let cache = &cache;
            let path = &path;
            scope.spawn(move || {
                let mut bytes = [0; 4];
                cache.read_range(path, offset, &mut bytes).unwrap();
                assert_eq!(&bytes, expected);
            });
        }
    });
}

#[test]
fn output_reopen_after_eviction_preserves_written_ranges() {
    let root = tempfile::tempdir().unwrap();
    let paths = Arc::new(vec![root.path().join("first"), root.path().join("second")]);
    let cache = OutputHandleCache::new(Arc::clone(&paths), Arc::new(vec![8, 8]), &[2, 1], 2);
    cache.state.lock().unwrap().max_open = 1;
    let control = OperationControl::default();
    write_file_at(&cache.get_or_open(0).unwrap(), 4, b"tail", &control).unwrap();
    drop(cache.get_or_open(1).unwrap());
    write_file_at(&cache.get_or_open(0).unwrap(), 0, b"head", &control).unwrap();
    assert_eq!(std::fs::read(&paths[0]).unwrap(), b"headtail");
}

#[test]
fn local_output_cache_expands_only_for_small_fully_shared_plans() {
    let root = tempfile::tempdir().unwrap();
    for (count, size, producers, workers, expected_limit) in [
        (1, 8, 1, 1, 64),
        (64, 8, 2, 4, 64),
        (65, 8, 2, 4, 8),
        (64, INDEPENDENT_OUTPUT_MIN_BYTES, 1, 4, 64),
        (64, INDEPENDENT_OUTPUT_MIN_BYTES, 2, 1, 64),
        (
            64,
            INDEPENDENT_OUTPUT_MIN_BYTES,
            2,
            4,
            if cfg!(windows) { 8 } else { 64 },
        ),
    ] {
        let paths = Arc::new(
            (0..count)
                .map(|index| root.path().join(index.to_string()))
                .collect(),
        );
        let mut sizes = vec![8; count];
        sizes[0] = size;
        let cache =
            OutputHandleCache::new(paths, Arc::new(sizes), &vec![producers; count], workers);
        assert_eq!(cache.local_handle_limit, expected_limit);
    }
}

#[test]
fn expanded_local_cache_reuses_global_file_objects_and_retains_the_large_plan_bound() {
    let root = tempfile::tempdir().unwrap();
    for count in [64, 65] {
        let paths = Arc::new(
            (0..count)
                .map(|index| root.path().join(format!("{count}-{index}")))
                .collect(),
        );
        let cache = OutputHandleCache::new(paths, Arc::new(vec![8; count]), &vec![2; count], 4);
        let mut first_job = HashMap::new();
        let mut second_job = HashMap::new();
        for index in 0..count {
            let first = cached_target_handle(&cache, &mut first_job, index).unwrap();
            let second = cached_target_handle(&cache, &mut second_job, index).unwrap();
            assert!(Arc::ptr_eq(&first, &second));
            let global = cache.state.lock().unwrap();
            assert!(Arc::ptr_eq(&first, &global.handles[&index]));
        }
        let expected_local = if count == 64 { 64 } else { 8 };
        assert_eq!(first_job.len(), expected_local);
        assert_eq!(second_job.len(), expected_local);
        assert_eq!(cache.state.lock().unwrap().handles.len(), count);
    }
}

#[test]
fn output_handle_selection_preserves_ranges_and_job_local_reuse() {
    for (size, producers, workers, independent) in [
        (4096, 2, 4, false),
        (INDEPENDENT_OUTPUT_MIN_BYTES - 1, 2, 4, false),
        (INDEPENDENT_OUTPUT_MIN_BYTES, 1, 4, false),
        (INDEPENDENT_OUTPUT_MIN_BYTES, 2, 1, false),
        (INDEPENDENT_OUTPUT_MIN_BYTES, 2, 4, cfg!(windows)),
    ] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("output");
        let cache = OutputHandleCache::new(
            Arc::new(vec![path.clone()]),
            Arc::new(vec![size]),
            &[producers],
            workers,
        );
        let mut first_job = HashMap::new();
        let first = cached_target_handle(&cache, &mut first_job, 0).unwrap();
        write_file_at(&first, size - 4, b"tail", &Default::default()).unwrap();
        let mut second_job = HashMap::new();
        let second = cached_target_handle(&cache, &mut second_job, 0).unwrap();
        assert_eq!(!Arc::ptr_eq(&first, &second), independent);
        assert!(Arc::ptr_eq(
            &first,
            &cached_target_handle(&cache, &mut first_job, 0).unwrap()
        ));
        std::thread::scope(|scope| {
            scope.spawn(|| write_file_at(&first, 0, b"head", &Default::default()).unwrap());
            scope.spawn(|| write_file_at(&second, 4, b"next", &Default::default()).unwrap());
        });
        let mut file = File::open(&path).unwrap();
        assert_eq!(file.metadata().unwrap().len(), size);
        let mut bytes = [0; 8];
        file.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"headnext");
        file.seek(SeekFrom::Start(size - 4)).unwrap();
        file.read_exact(&mut bytes[..4]).unwrap();
        assert_eq!(&bytes[..4], b"tail");
    }
}

//! Bounded file handles, buffered output ranges, and platform-specific positioned writes.
use crate::progress::OperationControl;
use crate::{Error, Result};
use std::collections::{HashMap, VecDeque};
use std::fs::{File, OpenOptions};
#[cfg(not(any(unix, windows)))]
use std::io::Write;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub(crate) const RECONSTRUCT_OUTPUT_BUFFER_BYTES: usize = 64 * 1024;

/// One reconstruction worker owns this buffer. Success requires an explicit flush;
/// failed or cancelled work discards buffered bytes instead of writing on drop.
pub(crate) struct ReconstructionOutputWriter<'a> {
    outputs: &'a OutputHandleCache,
    handles: HashMap<usize, Arc<File>>,
    control: &'a OperationControl,
    // The range fields are meaningful only while bytes is nonempty.
    file_index: usize,
    offset: u64,
    bytes: Vec<u8>,
}

impl<'a> ReconstructionOutputWriter<'a> {
    pub(crate) fn new(outputs: &'a OutputHandleCache, control: &'a OperationControl) -> Self {
        Self {
            outputs,
            handles: HashMap::new(),
            control,
            file_index: 0,
            offset: 0,
            bytes: Vec::new(),
        }
    }

    pub(crate) fn write(&mut self, file_index: usize, offset: u64, bytes: &[u8]) -> Result<()> {
        self.control.checkpoint()?;
        if !self.bytes.is_empty()
            && (self.file_index != file_index
                || self.offset.checked_add(self.bytes.len() as u64) != Some(offset)
                || self.bytes.len() + bytes.len() > RECONSTRUCT_OUTPUT_BUFFER_BYTES)
        {
            self.flush()?;
        }
        if bytes.len() >= RECONSTRUCT_OUTPUT_BUFFER_BYTES {
            return Self::write_range(
                self.outputs,
                &mut self.handles,
                self.control,
                file_index,
                offset,
                bytes,
            );
        }
        if self.bytes.is_empty() {
            self.file_index = file_index;
            self.offset = offset;
            if self.bytes.capacity() == 0 {
                self.bytes.reserve_exact(RECONSTRUCT_OUTPUT_BUFFER_BYTES);
            }
        }
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }

    pub(crate) fn write_unbuffered(
        &mut self,
        file_index: usize,
        offset: u64,
        bytes: &[u8],
    ) -> Result<()> {
        self.flush()?;
        Self::write_range(
            self.outputs,
            &mut self.handles,
            self.control,
            file_index,
            offset,
            bytes,
        )
    }

    pub(crate) fn flush(&mut self) -> Result<()> {
        if self.bytes.is_empty() {
            return Ok(());
        }
        Self::write_range(
            self.outputs,
            &mut self.handles,
            self.control,
            self.file_index,
            self.offset,
            &self.bytes,
        )?;
        self.bytes.clear();
        Ok(())
    }

    fn write_range(
        outputs: &OutputHandleCache,
        handles: &mut HashMap<usize, Arc<File>>,
        control: &OperationControl,
        file_index: usize,
        offset: u64,
        bytes: &[u8],
    ) -> Result<()> {
        control.checkpoint()?;
        let output = cached_target_handle(outputs, handles, file_index)?;
        write_file_at(&output, offset, bytes, control).map_err(|error| {
            error.with_io_context(
                "writing reconstructed range",
                outputs.output_path(file_index),
            )
        })
    }
}

pub(crate) struct OutputHandleCache {
    output_paths: Arc<Vec<PathBuf>>,
    output_sizes: Arc<Vec<u64>>,
    independent_handles: Vec<bool>,
    local_handle_limit: usize,
    state: Mutex<OutputHandleCacheState>,
}

#[derive(Default)]
pub(crate) struct SourceHandleCache {
    handles: Mutex<HashMap<PathBuf, Arc<Mutex<SourceReadHandle>>>>,
}
struct SourceReadHandle {
    file: File,
    position: Option<u64>,
}

impl SourceHandleCache {
    pub(crate) fn read_range(&self, path: &Path, offset: u64, bytes: &mut [u8]) -> Result<()> {
        let input = {
            let mut handles = self
                .handles
                .lock()
                .map_err(|_| Error::reconstruct("source cache poisoned"))?;
            if let Some(file) = handles.get(path) {
                Arc::clone(file)
            } else {
                if handles.len() >= 64
                    && let Some(key) = handles.keys().next().cloned()
                {
                    handles.remove(&key);
                }
                let file = Arc::new(Mutex::new(SourceReadHandle {
                    file: File::open(path)
                        .map_err(|e| Error::io_context("opening reused source file", path, e))?,
                    position: Some(0),
                }));
                handles.insert(path.to_path_buf(), Arc::clone(&file));
                file
            }
        };
        let mut input = input
            .lock()
            .map_err(|_| Error::reconstruct("source handle poisoned"))?;
        if input.position != Some(offset) {
            input.position = None;
            input
                .file
                .seek(SeekFrom::Start(offset))
                .map_err(|e| Error::io_context("seeking reused source file", path, e))?;
        }
        // A failed read can advance the cursor. Only a complete read establishes
        // a reusable position; the same handle mutex protects both values.
        input.position = None;
        input
            .file
            .read_exact(bytes)
            .map_err(|e| Error::io_context("reading reused source range", path, e))?;
        input.position = offset.checked_add(bytes.len() as u64);
        Ok(())
    }
}

struct OutputHandleCacheState {
    handles: HashMap<usize, Arc<File>>,
    lru: VecDeque<usize>,
    max_open: usize,
    initialized_files: Vec<bool>,
}

const LOCAL_OUTPUT_HANDLE_CACHE_LIMIT: usize = 8;
const SMALL_SHARED_OUTPUT_HANDLE_CACHE_LIMIT: usize = 64;
const INDEPENDENT_OUTPUT_MIN_BYTES: u64 = 64 * 1024 * 1024;

impl OutputHandleCache {
    pub(crate) fn output_path(&self, file_index: usize) -> &Path {
        &self.output_paths[file_index]
    }

    pub(crate) fn new(
        output_paths: Arc<Vec<PathBuf>>,
        output_sizes: Arc<Vec<u64>>,
        producer_counts: &[usize],
        worker_count: usize,
    ) -> Self {
        // These sizes and producer counts already exist for verification scheduling.
        // Independent Windows file objects help concurrent large-file writes, but
        // reopening tiny files costs more than it saves. No I/O probe is needed.
        let independent_handles: Vec<bool> = output_sizes
            .iter()
            .zip(producer_counts)
            .map(|(&size, &producers)| {
                cfg!(windows)
                    && worker_count > 1
                    && producers > 1
                    && size >= INDEPENDENT_OUTPUT_MIN_BYTES
            })
            .collect();
        // A small fully shared plan already fits in the global cache. Keeping its
        // references locally avoids repeated global lookups without opening more files.
        let local_handle_limit = if output_paths.len() <= SMALL_SHARED_OUTPUT_HANDLE_CACHE_LIMIT
            && !independent_handles.iter().any(|independent| *independent)
        {
            SMALL_SHARED_OUTPUT_HANDLE_CACHE_LIMIT
        } else {
            LOCAL_OUTPUT_HANDLE_CACHE_LIMIT
        };
        let initialized_files = vec![false; output_paths.len()];
        Self {
            output_paths,
            output_sizes,
            independent_handles,
            local_handle_limit,
            state: Mutex::new(OutputHandleCacheState {
                handles: HashMap::new(),
                lru: VecDeque::new(),
                max_open: 256,
                initialized_files,
            }),
        }
    }

    fn get_or_open(&self, file_index: usize) -> Result<Arc<File>> {
        if file_index >= self.output_paths.len() {
            return Err(Error::reconstruct(format!(
                "bundle target references unknown file index {}",
                file_index
            )));
        }
        let needs_initialization = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| Error::reconstruct("output handle cache is poisoned"))?;
            if let Some(file) = state.handles.get(&file_index).cloned() {
                state.lru.retain(|index| *index != file_index);
                state.lru.push_back(file_index);
                return Ok(file);
            }
            !state.initialized_files[file_index]
        };

        // Do filesystem work outside the cache mutex. A reconstruction can touch thousands of
        // files, and holding the mutex across open/set_len otherwise serializes every first write.
        let file = OpenOptions::new()
            .write(true)
            .create(needs_initialization)
            .truncate(false)
            .open(&self.output_paths[file_index])
            .map_err(|e| {
                Error::io_context(
                    "opening reconstruction output",
                    &self.output_paths[file_index],
                    e,
                )
            })?;
        if needs_initialization {
            file.set_len(self.output_sizes[file_index]).map_err(|e| {
                Error::io_context(
                    "sizing reconstruction output",
                    &self.output_paths[file_index],
                    e,
                )
            })?;
        }
        let file = Arc::new(file);

        let mut state = self
            .state
            .lock()
            .map_err(|_| Error::reconstruct("output handle cache is poisoned"))?;
        // Preserve successful initialization across handle eviction. Concurrent
        // first opens may both size the file, as before; failed sizing is not cached.
        state.initialized_files[file_index] = true;
        if let Some(existing) = state.handles.get(&file_index).cloned() {
            state.lru.retain(|index| *index != file_index);
            state.lru.push_back(file_index);
            return Ok(existing);
        }
        while state.handles.len() >= state.max_open {
            if let Some(oldest) = state.lru.pop_front() {
                state.handles.remove(&oldest);
            }
        }
        state.handles.insert(file_index, Arc::clone(&file));
        state.lru.push_back(file_index);
        Ok(file)
    }
}

pub(crate) fn cached_target_handle(
    output_handles: &OutputHandleCache,
    target_handles: &mut HashMap<usize, Arc<File>>,
    file_index: usize,
) -> Result<Arc<File>> {
    if let Some(file) = target_handles.get(&file_index) {
        return Ok(Arc::clone(file));
    }
    let file = output_handles.get_or_open(file_index)?;
    let file = if output_handles.independent_handles[file_index] {
        // Creation and sizing stay centralized. This handle only opens an already
        // prepared file; it must never truncate data written by another job.
        Arc::new(
            OpenOptions::new()
                .write(true)
                .open(&output_handles.output_paths[file_index])
                .map_err(|e| {
                    Error::io_context(
                        "opening independent output handle",
                        &output_handles.output_paths[file_index],
                        e,
                    )
                })?,
        )
    } else {
        file
    };
    if target_handles.len() >= output_handles.local_handle_limit
        && let Some(oldest_file_index) = target_handles.keys().next().copied()
    {
        target_handles.remove(&oldest_file_index);
    }
    target_handles.insert(file_index, Arc::clone(&file));
    Ok(file)
}

#[cfg(windows)]
pub(crate) fn write_file_at(
    file: &File,
    offset: u64,
    bytes: &[u8],
    control: &OperationControl,
) -> Result<()> {
    use std::os::windows::fs::FileExt;

    let mut written = 0usize;
    while written < bytes.len() {
        let count = file.seek_write(&bytes[written..], offset + written as u64)?;
        if count == 0 {
            return Err(Error::reconstruct("output file write returned zero bytes"));
        }
        written += count;
        control.written(count as u64);
    }
    Ok(())
}

#[cfg(unix)]
pub(crate) fn write_file_at(
    file: &File,
    offset: u64,
    bytes: &[u8],
    control: &OperationControl,
) -> Result<()> {
    use std::os::unix::fs::FileExt;

    let mut written = 0;
    while written < bytes.len() {
        let count = file.write_at(&bytes[written..], offset + written as u64)?;
        if count == 0 {
            return Err(Error::reconstruct("output write returned zero bytes"));
        }
        written += count;
        control.written(count as u64);
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn write_file_at(
    file: &File,
    offset: u64,
    bytes: &[u8],
    control: &OperationControl,
) -> Result<()> {
    let mut file = file.try_clone()?;
    file.seek(SeekFrom::Start(offset))?;
    let mut written = 0;
    while written < bytes.len() {
        let count = file.write(&bytes[written..])?;
        if count == 0 {
            return Err(Error::reconstruct("output write returned zero bytes"));
        }
        written += count;
        control.written(count as u64);
    }
    Ok(())
}

#[cfg(test)]
mod output_handle_tests {
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
        let mut writer = ReconstructionOutputWriter::new(&cache, &control);
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
        let mut writer = ReconstructionOutputWriter::new(&cache, &control);
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
        #[derive(Debug)]
        struct Cancellation(std::sync::atomic::AtomicBool);
        impl crate::progress::CancellationHook for Cancellation {
            fn is_cancelled(&self) -> bool {
                self.0.load(std::sync::atomic::Ordering::Relaxed)
            }
        }
        for cancel in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("missing-parent/output");
            let cache =
                OutputHandleCache::new(Arc::new(vec![path.clone()]), Arc::new(vec![8]), &[1], 1);
            let cancellation = Arc::new(Cancellation(std::sync::atomic::AtomicBool::new(false)));
            let control = OperationControl::default().with_cancellation(cancellation.clone());
            let mut writer = ReconstructionOutputWriter::new(&cache, &control);
            writer.write(0, 0, b"data").unwrap();
            cancellation
                .0
                .store(cancel, std::sync::atomic::Ordering::Relaxed);
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
        let cache =
            OutputHandleCache::new(Arc::new(vec![path.clone()]), Arc::new(vec![8]), &[1], 1);
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
}

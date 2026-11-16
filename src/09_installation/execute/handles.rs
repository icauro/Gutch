//! Bounded file handles, buffered output ranges, and platform-specific positioned writes.
use crate::error::IoContext;
use crate::progress::OperationControl;
use crate::{Error, Result};
use std::collections::{HashMap, VecDeque};
use std::fs::{File, OpenOptions};
#[cfg(not(any(unix, windows)))]
use std::io::Write;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

pub(crate) const RECONSTRUCT_OUTPUT_BUFFER_BYTES: usize = 64 * 1024;
/// Open reused-source files kept for whole-file copies.
const MAX_CACHED_SOURCE_HANDLES: usize = 64;
/// Open output files shared by reconstruction workers. Windows has no small
/// per-process limit, and closing a handle early only forces a reopen, which
/// contends with file creation; Unix soft limits can be as low as 256.
#[cfg(windows)]
const MAX_OPEN_OUTPUT_HANDLES: usize = 1024;
#[cfg(not(windows))]
const MAX_OPEN_OUTPUT_HANDLES: usize = 256;

/// One reconstruction worker owns this buffer. Success requires an explicit flush;
/// failed or cancelled work discards buffered bytes instead of writing on drop.
pub(crate) struct ReconstructOutputWriter<'a> {
    outputs: &'a OutputHandleCache,
    handles: HashMap<usize, Arc<File>>,
    control: &'a OperationControl,
    // The range fields are meaningful only while bytes is nonempty.
    file_index: usize,
    offset: u64,
    bytes: Vec<u8>,
}

impl<'a> ReconstructOutputWriter<'a> {
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
    /// Signals file preparation, first use of a prepared file, and stopping.
    changed: Condvar,
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
                if handles.len() >= MAX_CACHED_SOURCE_HANDLES
                    && let Some(key) = handles.keys().next().cloned()
                {
                    handles.remove(&key);
                }
                let file = Arc::new(Mutex::new(SourceReadHandle {
                    file: File::open(path).io_context("opening reused source file", path)?,
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
                .io_context("seeking reused source file", path)?;
        }
        // A failed read can advance the cursor. Only a complete read establishes
        // a reusable position; the same handle mutex protects both values.
        input.position = None;
        input
            .file
            .read_exact(bytes)
            .io_context("reading reused source range", path)?;
        input.position = offset.checked_add(bytes.len() as u64);
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OutputFileState {
    Unprepared,
    /// One thread is creating and sizing the file; others wait for it.
    Preparing,
    Prepared,
}

struct OutputHandleCacheState {
    handles: HashMap<usize, Arc<File>>,
    lru: VecDeque<usize>,
    max_open: usize,
    file_states: Vec<OutputFileState>,
    /// Files prepared ahead of the workers whose handles no worker has used yet.
    awaiting_use: Vec<bool>,
    /// Those files plus the ones being prepared ahead right now.
    awaiting_use_count: usize,
    stop_preparing: bool,
}

impl OutputHandleCacheState {
    fn touch(&mut self, file_index: usize) {
        self.lru.retain(|index| *index != file_index);
        self.lru.push_back(file_index);
    }

    /// Returns whether the file was still waiting for its first use.
    fn take_awaiting_use(&mut self, file_index: usize) -> bool {
        let awaiting = std::mem::replace(&mut self.awaiting_use[file_index], false);
        if awaiting {
            self.awaiting_use_count -= 1;
        }
        awaiting
    }

    /// Returns the evicted handles so callers close them outside the mutex.
    fn insert(&mut self, file_index: usize, file: Arc<File>) -> Vec<Arc<File>> {
        let mut evicted = Vec::new();
        while self.handles.len() >= self.max_open {
            let Some(oldest) = self.lru.pop_front() else {
                break;
            };
            self.take_awaiting_use(oldest);
            evicted.extend(self.handles.remove(&oldest));
        }
        self.handles.insert(file_index, file);
        self.lru.push_back(file_index);
        evicted
    }
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
        let file_count = output_paths.len();
        Self {
            output_paths,
            output_sizes,
            independent_handles,
            local_handle_limit,
            state: Mutex::new(OutputHandleCacheState {
                handles: HashMap::new(),
                lru: VecDeque::new(),
                max_open: MAX_OPEN_OUTPUT_HANDLES,
                file_states: vec![OutputFileState::Unprepared; file_count],
                awaiting_use: vec![false; file_count],
                awaiting_use_count: 0,
                stop_preparing: false,
            }),
            changed: Condvar::new(),
        }
    }

    fn lock_state(&self) -> Result<MutexGuard<'_, OutputHandleCacheState>> {
        self.state
            .lock()
            .map_err(|_| Error::reconstruct("output handle cache is poisoned"))
    }

    /// Creates and sizes files in `order` ahead of the workers, keeping at most
    /// half the cache in prepared files no worker has used yet. It stops on
    /// cancellation or the first failure; a worker then opens that file itself
    /// and reports the error.
    pub(crate) fn prepare_ahead(
        &self,
        order: &[usize],
        next: &AtomicUsize,
        control: &OperationControl,
    ) {
        loop {
            let Some(&file_index) = order.get(next.fetch_add(1, Ordering::Relaxed)) else {
                return;
            };
            {
                let Ok(mut state) = self.state.lock() else {
                    return;
                };
                while !state.stop_preparing && state.awaiting_use_count >= state.max_open / 2 {
                    let Ok(waited) = self.changed.wait(state) else {
                        return;
                    };
                    state = waited;
                }
                if state.stop_preparing || control.checkpoint().is_err() {
                    return;
                }
                if state.file_states[file_index] != OutputFileState::Unprepared {
                    continue;
                }
                state.file_states[file_index] = OutputFileState::Preparing;
                // Reserve the slot now so concurrent preparers respect the bound.
                state.awaiting_use_count += 1;
            }
            let opened = {
                let _create_span = control.profile_span("installation.reconstruct.output.create");
                self.open_output(file_index, true)
            };
            let Ok(mut state) = self.state.lock() else {
                return;
            };
            let Ok(file) = opened else {
                state.file_states[file_index] = OutputFileState::Unprepared;
                state.awaiting_use_count -= 1;
                self.changed.notify_all();
                return;
            };
            state.file_states[file_index] = OutputFileState::Prepared;
            let evicted = state.insert(file_index, Arc::new(file));
            state.awaiting_use[file_index] = true;
            self.changed.notify_all();
            drop(state);
            drop(evicted);
        }
    }

    /// Wakes and ends every [`Self::prepare_ahead`] loop.
    pub(crate) fn stop_preparing(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.stop_preparing = true;
        }
        self.changed.notify_all();
    }

    /// Closes a finished file's shared handle on the calling worker instead of
    /// at the end of reconstruction.
    pub(crate) fn release(&self, file_index: usize) -> Result<()> {
        let released = {
            let mut state = self.lock_state()?;
            state.take_awaiting_use(file_index);
            state.lru.retain(|index| *index != file_index);
            state.handles.remove(&file_index)
        };
        drop(released);
        Ok(())
    }

    fn open_output(&self, file_index: usize, create: bool) -> Result<File> {
        let path = &self.output_paths[file_index];
        let file = OpenOptions::new()
            .write(true)
            .create(create)
            .truncate(false)
            .open(path)
            .map_err(|e| Error::io_context("opening reconstruction output", path, e))?;
        if create {
            file.set_len(self.output_sizes[file_index])
                .map_err(|e| Error::io_context("sizing reconstruction output", path, e))?;
        }
        Ok(file)
    }

    fn get_or_open(&self, file_index: usize) -> Result<Arc<File>> {
        if file_index >= self.output_paths.len() {
            return Err(Error::reconstruct(format!(
                "bundle target references unknown file index {}",
                file_index
            )));
        }
        let create = {
            let mut state = self.lock_state()?;
            loop {
                if let Some(file) = state.handles.get(&file_index).cloned() {
                    state.touch(file_index);
                    if state.take_awaiting_use(file_index) {
                        self.changed.notify_all();
                    }
                    return Ok(file);
                }
                match state.file_states[file_index] {
                    OutputFileState::Preparing => {
                        state = self
                            .changed
                            .wait(state)
                            .map_err(|_| Error::reconstruct("output handle cache is poisoned"))?;
                    }
                    OutputFileState::Unprepared => {
                        state.file_states[file_index] = OutputFileState::Preparing;
                        break true;
                    }
                    OutputFileState::Prepared => break false,
                }
            }
        };

        // Do filesystem work outside the cache mutex. A reconstruction can touch thousands of
        // files, and holding the mutex across open/set_len otherwise serializes every first write.
        let opened = self.open_output(file_index, create);
        let mut state = self.lock_state()?;
        if create {
            // Failed sizing is not cached; the next writer creates the file again.
            state.file_states[file_index] = if opened.is_ok() {
                OutputFileState::Prepared
            } else {
                OutputFileState::Unprepared
            };
            self.changed.notify_all();
        }
        let file = Arc::new(opened?);
        if let Some(existing) = state.handles.get(&file_index).cloned() {
            state.touch(file_index);
            return Ok(existing);
        }
        let evicted = state.insert(file_index, Arc::clone(&file));
        drop(state);
        drop(evicted);
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
#[path = "handles_tests.rs"]
mod tests;

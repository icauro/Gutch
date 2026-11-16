//! Flushes staged artifacts to disk on dedicated threads while encoding continues.
//!
//! Encoders hand each staged file over as soon as it is written, so disk
//! write-back overlaps compression instead of following it at publication.

use std::fs;
use std::fs::File;
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::PoisonError;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::thread;
use std::thread::JoinHandle;

use crate::progress::OperationControl;

/// Owns the flush threads; dropping it without [`StagingFlusher::finish`]
/// skips queued work and joins the threads.
pub(crate) struct StagingFlusher {
    sender: Option<mpsc::Sender<File>>,
    workers: Vec<JoinHandle<()>>,
    shared: Arc<FlushState>,
}

#[derive(Default)]
struct FlushState {
    stop: AtomicBool,
    first_error: Mutex<Option<io::Error>>,
}

impl FlushState {
    fn record_error(&self, error: io::Error) {
        self.stop.store(true, Ordering::Relaxed);
        self.first_error
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_or_insert(error);
    }
}

impl StagingFlusher {
    pub(crate) fn new(thread_count: usize, control: &OperationControl) -> crate::Result<Self> {
        let (sender, receiver) = mpsc::channel::<File>();
        let receiver = Arc::new(Mutex::new(receiver));
        let mut flusher = Self {
            sender: Some(sender),
            workers: Vec::new(),
            shared: Arc::new(FlushState::default()),
        };
        for index in 0..thread_count.max(1) {
            let receiver = Arc::clone(&receiver);
            let shared = Arc::clone(&flusher.shared);
            let control = control.clone();
            // On a spawn failure, dropping `flusher` joins the threads already started.
            let worker = thread::Builder::new()
                .name(format!("Capture flush {index}"))
                .spawn(move || flush_until_closed(&receiver, &shared, &control))?;
            flusher.workers.push(worker);
        }
        Ok(flusher)
    }

    /// Takes the handle of a fully written staged file, then flushes and closes it.
    pub(crate) fn submit(&self, handle: File) {
        if let Some(sender) = &self.sender {
            // A send fails only once every worker has exited, which `finish` reports.
            let _ = sender.send(handle);
        }
    }

    /// Waits for every queued flush; afterwards each submitted file is durable.
    pub(crate) fn finish(mut self) -> crate::Result<()> {
        self.join();
        match self
            .shared
            .first_error
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            Some(error) => Err(error.into()),
            None => Ok(()),
        }
    }

    fn join(&mut self) {
        // Closing the channel lets each worker exit once the queue is empty.
        self.sender = None;
        for worker in self.workers.drain(..) {
            if worker.join().is_err() {
                self.shared
                    .record_error(io::Error::other("staged artifact flush panicked"));
            }
        }
    }
}

impl Drop for StagingFlusher {
    fn drop(&mut self) {
        if !self.workers.is_empty() {
            // The build failed or was cancelled; its staged files are discarded.
            self.shared.stop.store(true, Ordering::Relaxed);
            self.join();
        }
    }
}

fn flush_until_closed(
    receiver: &Mutex<mpsc::Receiver<File>>,
    shared: &FlushState,
    control: &OperationControl,
) {
    loop {
        let next = receiver
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .recv();
        let Ok(handle) = next else {
            return;
        };
        if shared.stop.load(Ordering::Relaxed) {
            continue;
        }
        let _span = control.profile_span("capture.artifact.flush_to_disk");
        if let Err(error) = handle.sync_all() {
            shared.record_error(error);
        }
        // Closing here keeps any work Windows does on close off the encoder threads.
        drop(handle);
    }
}

/// Windows flushes file buffers only through a handle opened for writing.
pub(crate) fn flush_file(path: &Path) -> io::Result<()> {
    fs::OpenOptions::new().write(true).open(path)?.sync_all()
}

#[cfg(test)]
#[path = "flush_tests.rs"]
mod tests;

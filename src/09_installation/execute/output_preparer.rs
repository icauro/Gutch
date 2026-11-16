//! Creates reconstruction output files on dedicated threads ahead of the
//! workers that write them.
//!
//! File creation contends inside the filesystem. On Windows NTFS, creation
//! throughput stopped rising at three or four threads, while twelve concurrent
//! creators each spent about ten times as long per file. A few dedicated threads
//! therefore create files in the order the workers will need them, and the
//! workers keep their time for decoding and writing.

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::thread;
use std::thread::JoinHandle;

use super::handles::OutputHandleCache;
use crate::progress::OperationControl;

const OUTPUT_PREPARER_THREADS: usize = 3;

/// Dropping it stops the preparation threads and joins them.
pub(crate) struct OutputFilePreparer {
    cache: Arc<OutputHandleCache>,
    threads: Vec<JoinHandle<()>>,
}

impl OutputFilePreparer {
    /// Starts preparing the files in `order`. Workers create any file the
    /// preparer has not reached yet themselves.
    pub(crate) fn start(
        cache: &Arc<OutputHandleCache>,
        order: Vec<usize>,
        control: &OperationControl,
    ) -> crate::Result<Self> {
        let order = Arc::new(order);
        let next = Arc::new(AtomicUsize::new(0));
        let mut preparer = Self {
            cache: Arc::clone(cache),
            threads: Vec::new(),
        };
        for index in 0..OUTPUT_PREPARER_THREADS.min(order.len()) {
            let cache = Arc::clone(cache);
            let order = Arc::clone(&order);
            let next = Arc::clone(&next);
            let control = control.clone();
            // On a spawn failure, dropping `preparer` joins the threads already started.
            let thread = thread::Builder::new()
                .name(format!("Reconstruct output {index}"))
                .spawn(move || cache.prepare_ahead(&order, &next, &control))?;
            preparer.threads.push(thread);
        }
        Ok(preparer)
    }
}

impl Drop for OutputFilePreparer {
    fn drop(&mut self) {
        self.cache.stop_preparing();
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

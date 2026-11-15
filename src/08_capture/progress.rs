use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use crate::progress::OperationControl;
use crate::progress::OperationKind;
use crate::progress::ProgressEvent;
use crate::progress::ProgressPhase;
use crate::progress::emit_progress;

pub(crate) struct BuildProgressTracker {
    control: OperationControl,
    phase: ProgressPhase,
    message: &'static str,
    current: AtomicUsize,
    total: AtomicUsize,
}

impl BuildProgressTracker {
    pub(crate) fn new(
        control: &OperationControl,
        phase: ProgressPhase,
        message: &'static str,
    ) -> Arc<Self> {
        Arc::new(Self {
            control: control.clone(),
            phase,
            message,
            current: AtomicUsize::new(0),
            total: AtomicUsize::new(0),
        })
    }

    pub(crate) fn start(&self, total: usize) {
        self.current.store(0, Ordering::Relaxed);
        self.total.store(total, Ordering::Relaxed);
        self.emit(0);
    }

    pub(crate) fn completed(&self) {
        let current = self.current.fetch_add(1, Ordering::Relaxed) + 1;
        self.emit(current);
    }

    fn emit(&self, current: usize) {
        emit_progress(
            self.control.progress(),
            ProgressEvent {
                feedback: None,
                operation: OperationKind::SnapshotCapture,
                phase: self.phase,
                message: self.message.to_string(),
                current,
                total: self.total.load(Ordering::Relaxed),
                transferred_bytes: 0,
                total_bytes: 0,
                detail: None,
                done: false,
            },
        );
    }
}

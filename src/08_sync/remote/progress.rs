//! Numbered step progress of a push or pull.

use crate::progress::OperationControl;
use crate::progress::OperationKind;
use crate::progress::ProgressEvent;
use crate::progress::ProgressPhase;
use crate::progress::ProgressSink;
use crate::progress::emit_progress;
use std::sync::Arc;

/// Reports the numbered steps of a push or pull.
pub(super) struct RemoteProgress {
    sink: Option<Arc<dyn ProgressSink>>,
    operation: OperationKind,
    step: usize,
    total_steps: usize,
}

impl RemoteProgress {
    pub(super) fn new(control: &OperationControl, operation: OperationKind) -> Self {
        Self {
            sink: control.progress().clone(),
            operation,
            step: 0,
            total_steps: 1,
        }
    }

    pub(super) fn set_total_steps(&mut self, total_steps: usize) {
        self.total_steps = total_steps;
    }

    /// Moves to the next step and reports it.
    pub(super) fn next_step(
        &mut self,
        phase: ProgressPhase,
        message: impl Into<String>,
        detail: Option<String>,
    ) {
        self.step += 1;
        self.report(phase, message, detail);
    }

    /// Reports progress within the current step.
    pub(super) fn report(
        &self,
        phase: ProgressPhase,
        message: impl Into<String>,
        detail: Option<String>,
    ) {
        self.emit(phase, message.into(), self.step, detail, false);
    }

    pub(super) fn finish(&self, message: &str) {
        self.emit(
            ProgressPhase::Done,
            message.to_string(),
            self.total_steps,
            None,
            true,
        );
    }

    fn emit(
        &self,
        phase: ProgressPhase,
        message: String,
        current: usize,
        detail: Option<String>,
        done: bool,
    ) {
        emit_progress(
            &self.sink,
            ProgressEvent {
                detail,
                done,
                ..ProgressEvent::new(self.operation, phase, message, current, self.total_steps)
            },
        );
    }
}

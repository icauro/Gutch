//! Cancellation and the controls carried through one operation.

use super::events::{OperationWarning, ProgressSink, WarningSink};
use super::feedback;
use super::feedback::Stream;
use crate::Error;
use crate::Result;
use crate::profiling::Arguments;
use crate::profiling::ProfileSpan;
use crate::profiling::Profiler;
use std::fmt::Debug;
use std::sync::Arc;

/// Read-only cancellation boundary implemented by the calling application.
///
/// Gutch deliberately does not own the mutable signal or depend on an async runtime. Long-running
/// engine code observes this hook only at consistency-safe checkpoints.
pub trait CancellationHook: Send + Sync + Debug {
    fn is_cancelled(&self) -> bool;
}

/// Cross-cutting controls propagated through one complete Gutch operation, including nested work.
#[derive(Clone, Debug, Default)]
pub struct OperationControl {
    progress: Option<Arc<dyn ProgressSink>>,
    cancellation: Option<Arc<dyn CancellationHook>>,
    profiler: Option<Arc<Profiler>>,
    feedback: Option<Arc<feedback::LiveProgress>>,
    warnings: Option<Arc<dyn WarningSink>>,
}

impl OperationControl {
    pub fn with_progress(mut self, progress: Arc<dyn ProgressSink>) -> Self {
        let feedback = feedback::LiveProgress::new(progress);
        if let Some(profiler) = &self.profiler {
            feedback.set_profiler(profiler.clone());
        }
        self.progress = Some(feedback.clone());
        self.feedback = Some(feedback);
        self
    }

    pub(crate) fn output_stage(&self) -> feedback::OutputStage {
        feedback::OutputStage::new(self.feedback.clone())
    }
    pub(crate) fn output_finished(&self) {
        if let Some(feedback) = &self.feedback {
            feedback.finish(Stream::OutputWrite);
        }
    }
    pub(crate) fn download_started(&self) {
        if let Some(f) = &self.feedback {
            f.start(Stream::Download);
        }
    }
    pub(crate) fn download_finished(&self) {
        if let Some(f) = &self.feedback {
            f.finish(Stream::Download);
        }
    }
    pub(crate) fn downloaded(&self, bytes: u64) {
        if let Some(f) = &self.feedback {
            f.record(Stream::Download, bytes);
        }
    }
    pub(crate) fn written(&self, bytes: u64) {
        if let Some(f) = &self.feedback {
            f.record(Stream::OutputWrite, bytes);
        }
    }
    pub fn with_cancellation(mut self, cancellation: Arc<dyn CancellationHook>) -> Self {
        self.cancellation = Some(cancellation);
        self
    }

    pub fn with_warnings(mut self, warnings: Arc<dyn WarningSink>) -> Self {
        self.warnings = Some(warnings);
        self
    }

    pub(crate) fn warn(&self, warning: OperationWarning) {
        if let Some(warnings) = &self.warnings {
            warnings.warn(warning);
        }
    }

    pub fn with_profiler(mut self, profiler: Arc<Profiler>) -> Self {
        if let Some(feedback) = &self.feedback {
            feedback.set_profiler(profiler.clone());
        }
        self.profiler = Some(profiler);
        self
    }

    /// Replaces only the progress adapter while retaining every other control.
    #[cfg(test)]
    pub(crate) fn with_replaced_progress(&self, progress: Arc<dyn ProgressSink>) -> Self {
        Self {
            progress: Some(progress),
            cancellation: self.cancellation.clone(),
            profiler: self.profiler.clone(),
            feedback: self.feedback.clone(),
            warnings: self.warnings.clone(),
        }
    }

    pub(crate) fn profile_span(&self, name: &'static str) -> Option<ProfileSpan> {
        self.profiler
            .as_ref()
            .and_then(|profiler| profiler.span(name))
    }

    pub(crate) fn profiler(&self) -> Option<&Profiler> {
        self.profiler.as_deref()
    }

    pub(crate) fn profile_span_with_args(
        &self,
        name: &'static str,
        args: impl FnOnce() -> Arguments,
    ) -> Option<ProfileSpan> {
        self.profiler
            .as_ref()
            .and_then(|profiler| profiler.span_with_args(name, args))
    }

    pub(crate) fn profile_event_with_args(
        &self,
        name: &'static str,
        args: impl FnOnce() -> Arguments,
    ) {
        if let Some(profiler) = &self.profiler {
            profiler.event_with_args(name, args);
        }
    }

    pub(crate) fn progress(&self) -> &Option<Arc<dyn ProgressSink>> {
        &self.progress
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancellation
            .as_ref()
            .is_some_and(|hook| hook.is_cancelled())
    }

    pub fn checkpoint(&self) -> Result<()> {
        if self.is_cancelled() {
            return Err(Error::Cancelled);
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "control_tests.rs"]
mod tests;

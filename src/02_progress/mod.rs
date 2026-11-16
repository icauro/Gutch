//! Operation control, cancellation, progress events, and bounded live feedback.

mod control;
mod events;
mod feedback;

pub use control::{CancelFlag, CancellationHook, OperationControl};
pub(crate) use events::emit_progress;
pub use events::{
    OperationKind, OperationWarning, ProgressEvent, ProgressPhase, ProgressSink, WarningSink,
};
pub use feedback::{LiveFeedback, Throughput};

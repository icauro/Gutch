//! Optional diagnostic profiling selected by `--profile`: span timings, a
//! bounded summary and, in detailed mode, a Perfetto trace.

mod profiler;
mod recording;
mod trace;
use profiler::{
    ARGUMENT_FIELDS, BLOCK_EVENTS, Event, OVERVIEW_BYTES, RECORDING_BYTES, Recorder,
    overview_marker,
};
pub(crate) use profiler::{Arguments, ProfileInstant, file_trace_args, trace_args};
pub use profiler::{ProfileMode, ProfileSpan, ProfileSpanSummary, ProfileSummary, Profiler};

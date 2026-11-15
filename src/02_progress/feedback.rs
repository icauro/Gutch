//! Coalesced operation feedback, independent of diagnostic profiling.
use super::{ProgressEvent, ProgressSink};
use crate::profiling::Arguments;
use crate::profiling::Profiler;
use serde::Serialize;
use std::collections::BTreeMap;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::Builder;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How often the feedback thread refreshes rates while a stream is active.
const REFRESH_INTERVAL: Duration = Duration::from_millis(250);
/// History that the recent rate covers.
const RECENT_WINDOW: Duration = Duration::from_secs(2);
/// One sample per refresh across the window, plus the window's two edges.
const MAX_SAMPLES: usize = (RECENT_WINDOW.as_millis() / REFRESH_INTERVAL.as_millis()) as usize + 2;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Throughput {
    pub bytes: u64,
    pub average_bytes_per_second: Option<f64>,
    pub recent_bytes_per_second: Option<f64>,
    pub finished: bool,
}
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveFeedback {
    pub download: Option<Throughput>,
    pub output_write: Option<Throughput>,
}
#[derive(Debug, Default)]
struct ThroughputHistory {
    start: Option<Instant>,
    end: Option<Instant>,
    samples: VecDeque<(Instant, u64)>,
}
impl ThroughputHistory {
    /// Starts the stream once; later starts keep the original history.
    fn start(&mut self, now: Instant) {
        if self.start.is_none() {
            self.start = Some(now);
            self.samples.push_back((now, 0));
        }
    }

    /// Ends a started stream once. Returns whether this call ended it.
    fn finish(&mut self, now: Instant) -> bool {
        let ends = self.start.is_some() && self.end.is_none();
        if ends {
            self.end = Some(now);
        }
        ends
    }

    fn snapshot(&mut self, bytes: u64, now: Instant) -> Option<Throughput> {
        let start = self.start?;
        let end = self.end.unwrap_or(now);
        if self.samples.back().is_none_or(|(at, _)| *at < end) {
            self.samples.push_back((end, bytes));
        }
        loop {
            let exceeds_sample_limit = self.samples.len() > MAX_SAMPLES;
            // Retain two samples even after a pause so a recent rate can be computed.
            let oldest_sample_outside_window = self.samples.len() > 2
                && end.saturating_duration_since(self.samples[1].0) > RECENT_WINDOW;
            if !exceeds_sample_limit && !oldest_sample_outside_window {
                break;
            }
            self.samples.pop_front();
        }
        let elapsed = end.saturating_duration_since(start).as_secs_f64();
        let (at, previous) = self.samples.front().copied().unwrap_or((start, 0));
        let recent_elapsed = end.saturating_duration_since(at).as_secs_f64();
        Some(Throughput {
            bytes,
            average_bytes_per_second: (elapsed > 0.0).then(|| bytes as f64 / elapsed),
            recent_bytes_per_second: (recent_elapsed > 0.0)
                .then(|| bytes.saturating_sub(previous) as f64 / recent_elapsed),
            finished: self.end.is_some(),
        })
    }
}
/// A byte stream whose throughput the live feedback reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stream {
    Download,
    OutputWrite,
}

#[derive(Debug, Default)]
struct FeedbackState {
    event: Option<ProgressEvent>,
    /// Counts every change worth emitting.
    revision: u64,
    /// Counts phase and completion changes, which are emitted without waiting.
    transitions: u64,
    stop: bool,
    download: ThroughputHistory,
    output: ThroughputHistory,
}
impl FeedbackState {
    fn history(&mut self, stream: Stream) -> &mut ThroughputHistory {
        match stream {
            Stream::Download => &mut self.download,
            Stream::OutputWrite => &mut self.output,
        }
    }
}
#[derive(Debug, Default)]
struct SharedFeedbackState {
    state: Mutex<FeedbackState>,
    changed: Condvar,
    downloaded: AtomicU64,
    written: AtomicU64,
    profiler: OnceLock<Arc<Profiler>>,
}

/// What the feedback thread last emitted.
#[derive(Default)]
struct Emitted {
    revision: u64,
    transitions: u64,
}
impl SharedFeedbackState {
    fn counter(&self, stream: Stream) -> &AtomicU64 {
        match stream {
            Stream::Download => &self.downloaded,
            Stream::OutputWrite => &self.written,
        }
    }

    /// Emits the latest event, with current rates, whenever it changes or a
    /// transfer is still active, until the feedback stops.
    fn run_feedback_loop(&self, sink: &dyn ProgressSink) {
        let mut emitted = Emitted::default();
        loop {
            let Some((event, stop)) = self.wait_for_event(&mut emitted) else {
                return;
            };
            if let Some(event) = event {
                self.profile_event(&event);
                sink.emit(event);
            }
            if stop {
                break;
            }
        }
    }

    /// Waits for a phase change or the refresh interval. Returns the event to
    /// emit, if any, and whether to stop; `None` when the state is poisoned.
    fn wait_for_event(&self, emitted: &mut Emitted) -> Option<(Option<ProgressEvent>, bool)> {
        let state = self.state.lock().ok()?;
        // A transition that arrived while the sink was busy needs no wait.
        let (mut state, _) = self
            .changed
            .wait_timeout_while(state, REFRESH_INTERVAL, |state| {
                !state.stop && state.transitions == emitted.transitions
            })
            .ok()?;
        emitted.transitions = state.transitions;
        let now = Instant::now();
        let feedback = LiveFeedback {
            download: state
                .download
                .snapshot(self.downloaded.load(Ordering::Relaxed), now),
            output_write: state
                .output
                .snapshot(self.written.load(Ordering::Relaxed), now),
        };
        let active = [&feedback.download, &feedback.output_write]
            .into_iter()
            .any(|rate| rate.as_ref().is_some_and(|rate| !rate.finished));
        let event = (state.revision != emitted.revision || active)
            .then(|| state.event.clone())
            .flatten()
            .map(|mut event| {
                event.feedback = Some(feedback);
                event
            });
        emitted.revision = state.revision;
        Some((event, state.stop))
    }

    fn profile_event(&self, event: &ProgressEvent) {
        let (Some(profiler), Some(feedback)) = (self.profiler.get(), &event.feedback) else {
            return;
        };
        profiler.event_with_args("operation.progress", || feedback_profile_args(feedback));
    }
}

fn feedback_profile_args(feedback: &LiveFeedback) -> Arguments {
    let mut args = BTreeMap::new();
    for (prefix, rate) in [
        ("download", &feedback.download),
        ("output_write", &feedback.output_write),
    ] {
        let Some(rate) = rate else {
            continue;
        };
        args.insert(format!("{prefix}_bytes"), serde_json::json!(rate.bytes));
        if let Some(recent) = rate.recent_bytes_per_second {
            args.insert(
                format!("{prefix}_bytes_per_second"),
                serde_json::json!(recent),
            );
        }
    }
    args
}

#[derive(Debug)]
pub(crate) struct LiveProgress {
    shared: Arc<SharedFeedbackState>,
    thread: Option<JoinHandle<()>>,
    /// Receives events directly when the feedback thread could not start.
    unthreaded_sink: Option<Arc<dyn ProgressSink>>,
}
impl LiveProgress {
    pub(crate) fn new(sink: Arc<dyn ProgressSink>) -> Arc<Self> {
        let shared = Arc::new(SharedFeedbackState::default());
        let thread_shared = shared.clone();
        let thread_sink = sink.clone();
        let thread = Builder::new()
            .name("gutch-progress".into())
            .spawn(move || thread_shared.run_feedback_loop(thread_sink.as_ref()))
            .ok();
        // Without the thread, events go straight through without live rates.
        let unthreaded_sink = thread.is_none().then_some(sink);
        Arc::new(Self {
            shared,
            thread,
            unthreaded_sink,
        })
    }
    /// Records profiling events for emitted feedback. The first profiler set
    /// is kept for every clone of the control.
    pub(crate) fn set_profiler(&self, profiler: Arc<Profiler>) {
        let _ = self.shared.profiler.set(profiler);
    }
    pub(crate) fn start(&self, stream: Stream) {
        if let Ok(mut state) = self.shared.state.lock() {
            state.history(stream).start(Instant::now());
        }
    }
    pub(crate) fn finish(&self, stream: Stream) {
        if let Ok(mut state) = self.shared.state.lock()
            && state.history(stream).finish(Instant::now())
        {
            state.revision += 1;
        }
    }
    pub(crate) fn record(&self, stream: Stream, bytes: u64) {
        self.shared
            .counter(stream)
            .fetch_add(bytes, Ordering::Relaxed);
    }
}
impl ProgressSink for LiveProgress {
    fn emit(&self, event: ProgressEvent) {
        if let Some(sink) = &self.unthreaded_sink {
            sink.emit(event);
            return;
        }
        if let Ok(mut state) = self.shared.state.lock() {
            let transition = state
                .event
                .as_ref()
                .is_none_or(|last| last.phase != event.phase || last.done != event.done);
            state.event = Some(event);
            state.revision += 1;
            if transition {
                state.transitions += 1;
                self.shared.changed.notify_one();
            }
        }
    }
}
impl Drop for LiveProgress {
    fn drop(&mut self) {
        if let Ok(mut state) = self.shared.state.lock() {
            state.stop = true;
            let now = Instant::now();
            // Only newly finished streams need a final event; the last one
            // already delivered is not repeated.
            let download_ended = state.download.finish(now);
            let output_ended = state.output.finish(now);
            if download_ended || output_ended {
                state.revision += 1;
            }
        }
        self.shared.changed.notify_one();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
pub(crate) struct OutputStage(Option<Arc<LiveProgress>>);
impl OutputStage {
    pub(crate) fn new(feedback: Option<Arc<LiveProgress>>) -> Self {
        if let Some(f) = &feedback {
            f.start(Stream::OutputWrite);
        }
        Self(feedback)
    }
}
impl Drop for OutputStage {
    fn drop(&mut self) {
        if let Some(f) = &self.0 {
            f.finish(Stream::OutputWrite);
        }
    }
}

#[cfg(test)]
#[path = "feedback_tests.rs"]
mod tests;

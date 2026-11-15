//! Optional diagnostic profiling selected by `--profile`: span timings, a
//! bounded summary and, in detailed mode, a Perfetto trace.

mod recording;
mod trace;

use serde::Serialize;
use std::collections::BTreeMap;
use std::io::{BufWriter, Write};
use std::marker::PhantomData;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use trace::TraceArguments;

pub(crate) type Arguments = BTreeMap<String, serde_json::Value>;
const RECORDING_BYTES: usize = 64 * 1024 * 1024;
const OVERVIEW_BYTES: usize = 1024 * 1024;
const ARGUMENT_FIELDS: usize = 8;
const BLOCK_EVENTS: usize = 32;
static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum ProfileMode {
    #[default]
    Off,
    Summary,
    Detailed,
}

/// Optional diagnostic clock. Scheduling and live progress use their own clocks.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ProfileInstant(Option<Instant>);
impl ProfileInstant {
    pub(crate) fn start(profiler: Option<&Profiler>) -> Self {
        Self(
            profiler
                .filter(|p| p.mode() != ProfileMode::Off)
                .map(|_| Instant::now()),
        )
    }
    pub(crate) fn detailed(profiler: Option<&Profiler>) -> Self {
        Self(
            profiler
                .filter(|p| p.mode() == ProfileMode::Detailed)
                .map(|_| Instant::now()),
        )
    }
    pub(crate) fn elapsed(self) -> Duration {
        self.0.map(|t| t.elapsed()).unwrap_or_default()
    }
    pub(crate) fn saturating_duration_since(self, earlier: Self) -> Duration {
        self.0
            .zip(earlier.0)
            .map(|(a, b)| a.saturating_duration_since(b))
            .unwrap_or_default()
    }
}

pub(crate) fn file_trace_args(path: impl AsRef<str>, bytes: u64) -> Arguments {
    let path = path.as_ref();
    // Trace arguments never contain absolute source paths.
    let path = if Path::new(path).is_absolute() {
        Path::new(path)
            .file_name()
            .and_then(|p| p.to_str())
            .unwrap_or("file")
    } else {
        path
    };
    BTreeMap::from([
        (
            "path".into(),
            serde_json::json!(path.chars().take(192).collect::<String>()),
        ),
        ("size_bytes".into(), serde_json::json!(bytes)),
    ])
}

#[derive(Debug, Clone)]
pub struct Profiler {
    state: Arc<State>,
}
#[derive(Debug)]
struct State {
    id: u64,
    mode: ProfileMode,
    started: Instant,
    recorders: Mutex<Vec<Arc<Mutex<Recorder>>>>,
    allocated: AtomicUsize,
    dropped: AtomicUsize,
    truncated: AtomicUsize,
    suppressed: AtomicUsize,
    detail_events: AtomicUsize,
    active_spans: AtomicUsize,
    finished: AtomicBool,
}
#[derive(Debug, Default)]
struct Recorder {
    tid: usize,
    thread_name: String,
    events: Vec<Event>,
    aggregates: BTreeMap<&'static str, Aggregate>,
}
#[derive(Debug)]
struct Event {
    name: &'static str,
    ts: u64,
    duration: Option<u64>,
    instant: bool,
    args: TraceArguments,
}

#[derive(Debug, Clone, Default)]
struct Aggregate {
    count: usize,
    total: Duration,
    maximum: Duration,
}
#[derive(Debug, Clone, Serialize)]
pub struct ProfileSpanSummary {
    pub name: &'static str,
    pub sampled: bool,
    pub count: usize,
    pub total_seconds: f64,
    pub maximum_seconds: f64,
}
#[derive(Debug, Clone, Serialize)]
pub struct ProfileSummary {
    pub mode: ProfileMode,
    pub span_count: usize,
    pub total_span_seconds: f64,
    pub spans: Vec<ProfileSpanSummary>,
    pub recording_limit_bytes: usize,
    pub recording_capacity_bytes: usize,
    pub dropped_events: usize,
    pub truncated_arguments: usize,
    pub suppressed_events: usize,
    pub detail_event_limit: usize,
}
pub struct ProfileSpan {
    recorder: Arc<Mutex<Recorder>>,
    state: Arc<State>,
    name: &'static str,
    started: Instant,
    event: Option<usize>,
    _thread_bound: PhantomData<Rc<()>>,
}

impl Profiler {
    pub fn new(mode: ProfileMode) -> Self {
        Self {
            state: Arc::new(State {
                id: NEXT_SESSION.fetch_add(1, Ordering::Relaxed),
                mode,
                started: Instant::now(),
                recorders: Mutex::new(Vec::new()),
                allocated: AtomicUsize::new(0),
                dropped: AtomicUsize::new(0),
                truncated: AtomicUsize::new(0),
                suppressed: AtomicUsize::new(0),
                detail_events: AtomicUsize::new(0),
                active_spans: AtomicUsize::new(0),
                finished: AtomicBool::new(false),
            }),
        }
    }
    pub fn mode(&self) -> ProfileMode {
        self.state.mode
    }
    pub(crate) fn span(&self, name: &'static str) -> Option<ProfileSpan> {
        self.span_with_args(name, BTreeMap::new)
    }
    pub(crate) fn span_with_args(
        &self,
        name: &'static str,
        args: impl FnOnce() -> Arguments,
    ) -> Option<ProfileSpan> {
        if self.mode() == ProfileMode::Off || self.state.finished.load(Ordering::Acquire) {
            return None;
        }
        let overview = overview_marker(name);
        if self.mode() == ProfileMode::Summary && !overview {
            return None;
        }
        let recorder = self.recorder()?;
        if !overview && !self.admit() {
            return None;
        }
        let started = Instant::now();
        let event = if self.mode() == ProfileMode::Detailed {
            self.reserve_event(&recorder, name, started, false, args)
        } else {
            None
        };
        self.state.active_spans.fetch_add(1, Ordering::AcqRel);
        Some(ProfileSpan {
            state: self.state.clone(),
            recorder,
            name,
            started,
            event,
            _thread_bound: PhantomData,
        })
    }
    pub(crate) fn event_with_args(&self, name: &'static str, args: impl FnOnce() -> Arguments) {
        if self.mode() != ProfileMode::Detailed || self.state.finished.load(Ordering::Acquire) {
            return;
        }
        if let Some(recorder) = self.recorder() {
            if !overview_marker(name) && !self.admit() {
                return;
            }
            self.reserve_event(&recorder, name, Instant::now(), true, args);
        }
    }
    pub fn summary(&self) -> ProfileSummary {
        let mut merged: BTreeMap<&'static str, Aggregate> = BTreeMap::new();
        if let Ok(recorders) = self.state.recorders.lock() {
            for recorder in recorders.iter() {
                if let Ok(recorder) = recorder.lock() {
                    for (&name, aggregate) in &recorder.aggregates {
                        let target = merged.entry(name).or_default();
                        target.count += aggregate.count;
                        target.total += aggregate.total;
                        target.maximum = target.maximum.max(aggregate.maximum);
                    }
                }
            }
        }
        let mut spans: Vec<_> = merged
            .into_iter()
            .map(|(name, a)| ProfileSpanSummary {
                name,
                sampled: !overview_marker(name)
                    && (self.state.suppressed.load(Ordering::Relaxed) > 0
                        || self.state.dropped.load(Ordering::Relaxed) > 0),
                count: a.count,
                total_seconds: a.total.as_secs_f64(),
                maximum_seconds: a.maximum.as_secs_f64(),
            })
            .collect();
        spans.sort_by(|a, b| b.total_seconds.total_cmp(&a.total_seconds));
        ProfileSummary {
            mode: self.mode(),
            span_count: spans.iter().map(|s| s.count).sum(),
            total_span_seconds: spans.iter().map(|s| s.total_seconds).sum(),
            spans,
            recording_limit_bytes: RECORDING_BYTES,
            recording_capacity_bytes: self.state.allocated.load(Ordering::Relaxed),
            dropped_events: self.state.dropped.load(Ordering::Relaxed),
            truncated_arguments: self.state.truncated.load(Ordering::Relaxed),
            suppressed_events: self.state.suppressed.load(Ordering::Relaxed),
            detail_event_limit: 100000,
        }
    }
    /// Export after joining producers and dropping operation controls and span guards.
    pub fn finish(&self, directory: &Path, operation_succeeded: bool) -> crate::Result<()> {
        if self.state.active_spans.load(Ordering::Acquire) != 0 {
            return Err(crate::Error::manifest(
                "join profiling workers and close spans before exporting",
            ));
        }
        self.state.finished.store(true, Ordering::Release);
        std::fs::create_dir_all(directory)?;
        let started = Instant::now();
        let trace_result = if self.mode() == ProfileMode::Detailed {
            self.write_trace(&directory.join("trace.json"))
        } else {
            Ok(())
        };
        let report = serde_json::json!({
            "profile": self.summary(),
            "operation_succeeded": operation_succeeded,
            "export_succeeded": trace_result.is_ok(),
            "trace_export_seconds": started.elapsed().as_secs_f64(),
            "duration_semantics": "inclusive spans; nested spans overlap; recording limits may omit jobs",
        });
        let mut summary_output =
            BufWriter::new(std::fs::File::create(directory.join("summary.json"))?);
        serde_json::to_writer_pretty(&mut summary_output, &report)?;
        summary_output.flush()?;
        trace_result
    }
}
impl Drop for ProfileSpan {
    fn drop(&mut self) {
        let finished = Instant::now();
        let duration = finished.duration_since(self.started);
        if let Ok(mut recorder) = self.recorder.lock() {
            if recorder.aggregates.contains_key(self.name) || recorder.aggregates.len() < 256 {
                let aggregate = recorder.aggregates.entry(self.name).or_default();
                aggregate.count += 1;
                aggregate.total += duration;
                aggregate.maximum = aggregate.maximum.max(duration);
            }
            if let Some(index) = self.event {
                // Quantize both endpoints on the session clock. Rounding the duration
                // separately can make a nested child end one microsecond after its parent.
                let end = finished.duration_since(self.state.started).as_micros() as u64;
                let event = &mut recorder.events[index];
                event.duration = Some(end - event.ts);
            }
        }
        self.state.active_spans.fetch_sub(1, Ordering::Release);
    }
}

/// Summary keeps workflow boundaries; detailed job records use the detail budget.
fn overview_marker(name: &str) -> bool {
    matches!(
        name,
        "cli.command"
            | "repo.status"
            | "transfer.concurrency"
            | "operation.progress"
            | "transfer.fetch"
            | "transfer.fetch_reconstruct"
            | "installation.reconstruct"
            | "installation.reconstruct.apply"
            | "installation.reconstruct.pipeline"
            | "installation.reconstruct.verify"
            | "installation.verify"
            | "installation.repair"
            | "installation.repair.verify"
            | "installation.repair.finalize"
            | "installation.corrupt"
            | "capture.prepare_source"
            | "capture.build_snapshot"
            | "capture.scan_source"
            | "capture.scan_source.inventory"
            | "capture.plan_deduplicated_chunks"
            | "capture.build_artifacts"
            | "capture.build_snapshot_manifest"
            | "capture.publish_snapshot"
            | "transfer.prepare"
            | "transfer.download_queue"
    ) || [
        "installation.verify.",
        "installation.repair.verify.",
        "installation.reconstruct.inspect",
    ]
    .iter()
    .any(|prefix| name.starts_with(prefix) && !name.ends_with(".file"))
        || name == "capture.create_worker_pool"
        || name.starts_with("installation.reconstruct.prepare.") && !name.ends_with("directory")
        || name.starts_with("installation.reconstruct.finalize.")
        || name.starts_with("capture.prepare_source.")
        || name.starts_with("capture.publish_snapshot.")
            && !name.contains("write_bundle")
            && !name.contains("write_loose")
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;

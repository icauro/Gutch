use serde::Serialize;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::io::{BufWriter, Write};
use std::marker::PhantomData;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

pub(crate) type Arguments = BTreeMap<String, serde_json::Value>;
const RECORDING_BYTES: usize = 64 * 1024 * 1024;
const OVERVIEW_BYTES: usize = 1024 * 1024;
const ARGUMENT_FIELDS: usize = 8;
const BLOCK_EVENTS: usize = 32;
static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);
thread_local! {
    static RECORDERS: RefCell<HashMap<u64, Weak<Mutex<Recorder>>>> = RefCell::new(HashMap::new());
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum GutchProfileMode {
    #[default]
    Off,
    Summary,
    Detailed,
}

/// Optional diagnostic clock. Scheduling and live progress use their own clocks.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ProfileInstant(Option<Instant>);
impl ProfileInstant {
    pub(crate) fn start(profiler: Option<&GutchProfiler>) -> Self {
        Self(
            profiler
                .filter(|p| p.mode() != GutchProfileMode::Off)
                .map(|_| Instant::now()),
        )
    }
    pub(crate) fn detailed(profiler: Option<&GutchProfiler>) -> Self {
        Self(
            profiler
                .filter(|p| p.mode() == GutchProfileMode::Detailed)
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
pub struct GutchProfiler {
    state: Arc<State>,
}
#[derive(Debug)]
struct State {
    id: u64,
    mode: GutchProfileMode,
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
#[derive(Debug)]
struct TraceText<const N: usize> {
    bytes: [u8; N],
    len: usize,
}
impl<const N: usize> TraceText<N> {
    fn new(value: &str) -> Self {
        let mut len = value.len().min(N);
        while !value.is_char_boundary(len) {
            len -= 1;
        }
        let mut bytes = [0; N];
        bytes[..len].copy_from_slice(&value.as_bytes()[..len]);
        Self { bytes, len }
    }
    fn as_str(&self) -> &str {
        std::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }
}
impl<const N: usize> Serialize for TraceText<N> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum TraceValue {
    Unsigned(u64),
    Signed(i64),
    Float(f64),
    Boolean(bool),
    Text(TraceText<192>),
}
#[derive(Debug)]
struct TraceField {
    key: TraceText<48>,
    value: TraceValue,
}
#[derive(Debug)]
struct TraceArguments([Option<TraceField>; ARGUMENT_FIELDS]);
impl Serialize for TraceArguments {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(None)?;
        for field in self.0.iter().flatten() {
            map.serialize_entry(&field.key, &field.value)?;
        }
        map.end()
    }
}

#[derive(Debug, Clone, Default)]
struct Aggregate {
    count: usize,
    total: Duration,
    maximum: Duration,
}
#[derive(Debug, Clone, Serialize)]
pub struct GutchProfileSpanSummary {
    pub name: &'static str,
    pub sampled: bool,
    pub count: usize,
    pub total_seconds: f64,
    pub maximum_seconds: f64,
}
#[derive(Debug, Clone, Serialize)]
pub struct GutchProfileSummary {
    pub mode: GutchProfileMode,
    pub span_count: usize,
    pub total_span_seconds: f64,
    pub spans: Vec<GutchProfileSpanSummary>,
    pub recording_limit_bytes: usize,
    pub recording_capacity_bytes: usize,
    pub dropped_events: usize,
    pub truncated_arguments: usize,
    pub suppressed_events: usize,
    pub detail_event_limit: usize,
}
pub struct GutchProfileSpan {
    recorder: Arc<Mutex<Recorder>>,
    state: Arc<State>,
    name: &'static str,
    started: Instant,
    event: Option<usize>,
    _thread_bound: PhantomData<Rc<()>>,
}

impl GutchProfiler {
    pub fn new(mode: GutchProfileMode) -> Self {
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
    pub fn mode(&self) -> GutchProfileMode {
        self.state.mode
    }
    fn recorder(&self) -> Option<Arc<Mutex<Recorder>>> {
        RECORDERS.with(|cache| {
            let mut cache = cache.try_borrow_mut().ok()?;
            if let Some(recorder) = cache.get(&self.state.id).and_then(Weak::upgrade) {
                return Some(recorder);
            }
            cache.retain(|_, entry| entry.strong_count() > 0);
            let mut registered = self.state.recorders.lock().ok()?;
            // Covers registration, bounded static-name maps, and local bookkeeping.
            if self
                .state
                .allocated
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                    n.checked_add(65536)
                        .filter(|n| *n <= RECORDING_BYTES - OVERVIEW_BYTES)
                })
                .is_err()
            {
                self.state.dropped.fetch_add(1, Ordering::Relaxed);
                return None;
            }
            let recorder = Arc::new(Mutex::new(Recorder {
                tid: registered.len() + 1,
                thread_name: std::thread::current()
                    .name()
                    .unwrap_or("gutch-worker")
                    .chars()
                    .take(96)
                    .collect(),
                ..Default::default()
            }));
            registered.push(recorder.clone());
            cache.insert(self.state.id, Arc::downgrade(&recorder));
            Some(recorder)
        })
    }
    pub(crate) fn span(&self, name: &'static str) -> Option<GutchProfileSpan> {
        self.span_with_args(name, BTreeMap::new)
    }
    pub(crate) fn span_with_args(
        &self,
        name: &'static str,
        args: impl FnOnce() -> Arguments,
    ) -> Option<GutchProfileSpan> {
        if self.mode() == GutchProfileMode::Off || self.state.finished.load(Ordering::Acquire) {
            return None;
        }
        let overview = overview_marker(name);
        if self.mode() == GutchProfileMode::Summary && !overview {
            return None;
        }
        let recorder = self.recorder()?;
        if !overview && !self.admit() {
            return None;
        }
        let started = Instant::now();
        let event = if self.mode() == GutchProfileMode::Detailed {
            self.reserve_event(&recorder, name, started, false, args)
        } else {
            None
        };
        self.state.active_spans.fetch_add(1, Ordering::AcqRel);
        Some(GutchProfileSpan {
            state: self.state.clone(),
            recorder,
            name,
            started,
            event,
            _thread_bound: PhantomData,
        })
    }
    pub(crate) fn event_with_args(&self, name: &'static str, args: impl FnOnce() -> Arguments) {
        if self.mode() != GutchProfileMode::Detailed || self.state.finished.load(Ordering::Acquire)
        {
            return;
        }
        if let Some(recorder) = self.recorder() {
            if !overview_marker(name) && !self.admit() {
                return;
            }
            self.reserve_event(&recorder, name, Instant::now(), true, args);
        }
    }
    fn admit(&self) -> bool {
        // Detailed traces record consecutive jobs; time-based sampling makes busy
        // workers look idle. The event and byte budgets still bound recording.
        if self
            .state
            .detail_events
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                (n < 100000).then_some(n + 1)
            })
            .is_err()
        {
            self.state.suppressed.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        true
    }
    fn reserve_event(
        &self,
        recorder: &Arc<Mutex<Recorder>>,
        name: &'static str,
        started: Instant,
        instant: bool,
        args: impl FnOnce() -> Arguments,
    ) -> Option<usize> {
        let mut recorder = recorder.lock().ok()?;
        if recorder.events.len() == recorder.events.capacity() {
            let bytes = BLOCK_EVENTS * std::mem::size_of::<Event>();
            let limit = if overview_marker(name) {
                RECORDING_BYTES
            } else {
                RECORDING_BYTES - OVERVIEW_BYTES
            };
            if self
                .state
                .allocated
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                    n.checked_add(bytes).filter(|n| *n <= limit)
                })
                .is_err()
            {
                self.state.dropped.fetch_add(1, Ordering::Relaxed);
                return None;
            }
            recorder.events.reserve_exact(BLOCK_EVENTS);
        }
        let mut fields = std::array::from_fn(|_| None);
        let mut index = 0;
        for (key, value) in args() {
            if key.contains("source_path") || key.contains("url") {
                continue;
            }
            if index == ARGUMENT_FIELDS {
                self.state.truncated.fetch_add(1, Ordering::Relaxed);
                break;
            }
            let value = match value {
                serde_json::Value::Bool(v) => TraceValue::Boolean(v),
                serde_json::Value::Number(v) => {
                    if let Some(v) = v.as_u64() {
                        TraceValue::Unsigned(v)
                    } else if let Some(v) = v.as_i64() {
                        TraceValue::Signed(v)
                    } else {
                        TraceValue::Float(v.as_f64().unwrap_or(0.0))
                    }
                }
                serde_json::Value::String(v) => {
                    if v.len() > 192 {
                        self.state.truncated.fetch_add(1, Ordering::Relaxed);
                    }
                    TraceValue::Text(TraceText::new(&v))
                }
                _ => {
                    self.state.truncated.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
            };
            if key.len() > 48 {
                self.state.truncated.fetch_add(1, Ordering::Relaxed);
            }
            fields[index] = Some(TraceField {
                key: TraceText::new(&key),
                value,
            });
            index += 1;
        }
        let event = Event {
            name,
            ts: started.duration_since(self.state.started).as_micros() as u64,
            duration: instant.then_some(0),
            instant,
            args: TraceArguments(fields),
        };
        let index = recorder.events.len();
        recorder.events.push(event);
        Some(index)
    }
    pub fn summary(&self) -> GutchProfileSummary {
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
            .map(|(name, a)| GutchProfileSpanSummary {
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
        GutchProfileSummary {
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
        let trace_result = if self.mode() == GutchProfileMode::Detailed {
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
    pub(crate) fn write_trace(&self, path: &Path) -> crate::Result<()> {
        let mut output = BufWriter::new(std::fs::File::create(path)?);
        output.write_all(b"{\"displayTimeUnit\":\"ms\",\"traceEvents\":[")?;
        let process_name = path
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .unwrap_or("gutch");
        let process_metadata = serde_json::json!({
            "name": "process_name", "cat": "__metadata", "ph": "M",
            "ts": 0, "pid": 1, "tid": 0, "args": { "name": process_name },
        });
        serde_json::to_writer(&mut output, &process_metadata)?;
        let dropped = self.state.dropped.load(Ordering::Relaxed);
        let suppressed = self.state.suppressed.load(Ordering::Relaxed);
        if dropped > 0 || suppressed > 0 {
            output.write_all(b",")?;
            serde_json::to_writer(
                &mut output,
                &serde_json::json!({
                    "name": "profile.recording_incomplete", "cat": "gutch", "ph": "i",
                    "s": "g", "ts": 0, "pid": 1, "tid": 0,
                    "args": {"dropped_events": dropped, "suppressed_events": suppressed},
                }),
            )?;
        }
        if let Ok(recorders) = self.state.recorders.lock() {
            for recorder in recorders.iter() {
                if let Ok(recorder) = recorder.lock() {
                    output.write_all(b",")?;
                    let thread_metadata = serde_json::json!({
                        "name": "thread_name", "cat": "__metadata", "ph": "M",
                        "ts": 0, "pid": 1, "tid": recorder.tid,
                        "args": { "name": recorder.thread_name },
                    });
                    serde_json::to_writer(&mut output, &thread_metadata)?;
                    for event in &recorder.events {
                        if let Some(duration) = event.duration {
                            output.write_all(b",")?;
                            #[derive(Serialize)]
                            struct ExportEvent<'a> {
                                name: &'static str,
                                cat: &'static str,
                                ph: &'static str,
                                ts: u64,
                                dur: u64,
                                pid: u32,
                                tid: usize,
                                args: &'a TraceArguments,
                                #[serde(skip_serializing_if = "Option::is_none")]
                                s: Option<&'static str>,
                            }
                            serde_json::to_writer(
                                &mut output,
                                &ExportEvent {
                                    name: event.name,
                                    cat: "gutch",
                                    ph: if event.name == "operation.progress" {
                                        "C"
                                    } else if event.instant {
                                        "i"
                                    } else {
                                        "X"
                                    },
                                    ts: event.ts,
                                    dur: duration,
                                    pid: 1,
                                    tid: recorder.tid,
                                    args: &event.args,
                                    s: (event.instant && event.name != "operation.progress")
                                        .then_some("t"),
                                },
                            )?;
                        }
                    }
                }
            }
        }
        output.write_all(b"]}")?;
        output.flush()?;
        Ok(())
    }
}
impl Drop for GutchProfileSpan {
    fn drop(&mut self) {
        let duration = self.started.elapsed();
        if let Ok(mut recorder) = self.recorder.lock() {
            if recorder.aggregates.contains_key(self.name) || recorder.aggregates.len() < 256 {
                let aggregate = recorder.aggregates.entry(self.name).or_default();
                aggregate.count += 1;
                aggregate.total += duration;
                aggregate.maximum = aggregate.maximum.max(duration);
            }
            if let Some(index) = self.event {
                recorder.events[index].duration = Some(duration.as_micros() as u64);
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
mod tests {
    use super::*;
    #[test]
    fn disabled_and_summary_do_not_evaluate_arguments() {
        for mode in [GutchProfileMode::Off, GutchProfileMode::Summary] {
            let p = GutchProfiler::new(mode);
            drop(p.span_with_args("test", || panic!("arguments evaluated")));
            p.event_with_args("event", || panic!("arguments evaluated"));
        }
    }
    #[test]
    fn millions_of_job_markers_do_not_build_millions_of_arguments() {
        let p = GutchProfiler::new(GutchProfileMode::Detailed);
        let evaluated = std::cell::Cell::new(0usize);
        for _ in 0..1_000_000 {
            drop(p.span_with_args("capture.scan.file", || {
                evaluated.set(evaluated.get() + 1);
                BTreeMap::new()
            }));
        }
        drop(p.span("capture.publish_snapshot"));
        let summary = p.summary();
        assert!(evaluated.get() <= 100000);
        assert!(summary.suppressed_events >= 900_000);
        assert!(summary.recording_capacity_bytes <= summary.recording_limit_bytes);
        assert!(
            summary
                .spans
                .iter()
                .any(|s| s.name == "capture.publish_snapshot" && !s.sampled)
        );
    }
    #[test]
    fn exhausted_detail_budget_preserves_overview_and_skips_arguments() {
        let p = GutchProfiler::new(GutchProfileMode::Detailed);
        let recorder = p.recorder().unwrap();
        p.state
            .allocated
            .store(RECORDING_BYTES - OVERVIEW_BYTES, Ordering::Relaxed);
        drop(p.span_with_args("capture.scan.file", || {
            panic!("exhausted argument evaluated")
        }));
        drop(p.span("capture.publish_snapshot"));
        let recorder = recorder.lock().unwrap();
        assert!(
            recorder
                .events
                .iter()
                .any(|e| e.name == "capture.publish_snapshot")
        );
        drop(recorder);
        let directory = tempfile::tempdir().unwrap();
        p.finish(directory.path(), true).unwrap();
        let trace: serde_json::Value = serde_json::from_slice(
            &std::fs::read(directory.path().join("trace.json")).unwrap(),
        )
        .unwrap();
        assert!(trace["traceEvents"].as_array().unwrap().iter().any(|event| {
            event["name"] == "profile.recording_incomplete"
                && event["args"]["dropped_events"].as_u64().unwrap() > 0
        }));
    }
    #[test]
    fn consecutive_jobs_are_recorded_without_time_sampling() {
        let profiler = GutchProfiler::new(GutchProfileMode::Detailed);
        for _ in 0..512 {
            drop(profiler.span("capture.scan.file"));
            drop(profiler.span("capture.bundle.encode_and_write"));
        }
        let summary = profiler.summary();
        assert_eq!(summary.spans.len(), 2);
        assert!(summary.spans.iter().all(|span| span.count == 512 && !span.sampled));
        assert_eq!(summary.suppressed_events, 0);
        assert_eq!(summary.dropped_events, 0);
        let recorder = profiler.recorder().unwrap();
        let recorder = recorder.lock().unwrap();
        assert_eq!(recorder.events.len(), 1024);
        assert!(recorder.events.iter().all(|event| event.duration.is_some()));
        assert!(ProfileInstant::start(None).0.is_none());
        let summary = GutchProfiler::new(GutchProfileMode::Summary);
        assert!(ProfileInstant::detailed(Some(&summary)).0.is_none());
    }
    #[test]
    fn finalization_rejects_live_spans_and_reports_export_failure() {
        let profiler = GutchProfiler::new(GutchProfileMode::Detailed);
        let directory = tempfile::tempdir().unwrap();
        let span = profiler.span("cli.command");
        assert!(profiler.finish(directory.path(), true).is_err());
        drop(span);
        std::fs::create_dir(directory.path().join("trace.json")).unwrap();
        assert!(profiler.finish(directory.path(), true).is_err());
        let report: serde_json::Value =
            serde_json::from_slice(&std::fs::read(directory.path().join("summary.json")).unwrap())
                .unwrap();
        assert_eq!(report["operation_succeeded"], true);
        assert_eq!(report["export_succeeded"], false);
        assert!(profiler.span("capture.build_snapshot").is_none());
    }
    #[test]
    fn retained_arguments_are_bounded() {
        let profiler = GutchProfiler::new(GutchProfileMode::Detailed);
        drop(profiler.span_with_args("capture.scan.file", || {
            (0..100)
                .map(|index| {
                    (
                        format!("field_{index}"),
                        serde_json::json!("x".repeat(10000)),
                    )
                })
                .collect()
        }));
        let summary = profiler.summary();
        assert!(summary.truncated_arguments > 0);
        assert!(summary.recording_capacity_bytes < 1024 * 1024);
        let recorder = profiler.recorder().unwrap();
        let recorder = recorder.lock().unwrap();
        assert_eq!(
            recorder.events[0].args.0.iter().flatten().count(),
            ARGUMENT_FIELDS
        );
    }
    #[test]
    fn sessions_are_isolated_and_export_complete_events() {
        let a = GutchProfiler::new(GutchProfileMode::Detailed);
        let b = GutchProfiler::new(GutchProfileMode::Summary);
        drop(a.span("a"));
        drop(b.span("capture.build_snapshot"));
        assert_eq!(a.summary().spans[0].name, "a");
        assert_eq!(b.summary().spans[0].name, "capture.build_snapshot");
        let dir = tempfile::tempdir().unwrap();
        a.finish(dir.path(), true).unwrap();
        let trace: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.path().join("trace.json")).unwrap()).unwrap();
        assert!(
            trace["traceEvents"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["ph"] == "X")
        );
    }
}

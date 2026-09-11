use std::collections::BTreeMap;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use serde::Serialize;

const TRACE_CATEGORY: &str = "gutch";
const BYTES_PER_MIB: f64 = 1024.0 * 1024.0;

pub(crate) fn file_trace_args(
    path: impl Into<String>,
    size_bytes: u64,
) -> BTreeMap<String, serde_json::Value> {
    BTreeMap::from([
        ("path".to_string(), serde_json::json!(path.into())),
        (
            "size_mib".to_string(),
            serde_json::json!(size_bytes as f64 / BYTES_PER_MIB),
        ),
    ])
}

#[derive(Debug, Clone)]
pub struct GutchProfiler {
    state: Arc<ProfilerState>,
}

#[derive(Debug)]
struct ProfilerState {
    started_at: Instant,
    process_id: u32,
    events: Mutex<Vec<TraceEvent>>,
    aggregates: Mutex<BTreeMap<String, SpanAggregate>>,
    thread_ids: Mutex<HashMap<std::thread::ThreadId, u64>>,
}

#[derive(Debug, Clone, Serialize)]
struct TraceEvent {
    name: String,
    cat: &'static str,
    ph: &'static str,
    ts: u64,
    pid: u32,
    tid: u64,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    args: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Default)]
struct SpanAggregate {
    count: usize,
    total: Duration,
    maximum: Duration,
}

#[derive(Debug, Clone, Serialize)]
pub struct GutchProfileSpanSummary {
    pub name: String,
    pub count: usize,
    pub total_seconds: f64,
    pub maximum_seconds: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct GutchProfileSummary {
    pub span_count: usize,
    pub total_span_seconds: f64,
    pub spans: Vec<GutchProfileSpanSummary>,
}

#[derive(Debug, Serialize)]
struct TraceDocument {
    #[serde(rename = "traceEvents")]
    trace_events: Vec<TraceEvent>,
    #[serde(rename = "displayTimeUnit")]
    display_time_unit: &'static str,
}

pub struct GutchProfileSpan {
    profiler: Option<GutchProfiler>,
    name: String,
    started_at: Instant,
    process_id: u32,
    thread_id: u64,
}

impl GutchProfiler {
    pub fn new() -> Self {
        let process_id = std::process::id();
        let state = ProfilerState {
            started_at: Instant::now(),
            process_id,
            events: Mutex::new(Vec::new()),
            aggregates: Mutex::new(BTreeMap::new()),
            thread_ids: Mutex::new(HashMap::new()),
        };
        let profiler = Self {
            state: Arc::new(state),
        };
        profiler.metadata_event(
            "process_name",
            BTreeMap::from([(String::from("name"), serde_json::json!("gutch"))]),
        );
        profiler
    }

    pub fn span(&self, name: impl Into<String>) -> GutchProfileSpan {
        self.span_with_args(name, BTreeMap::new())
    }

    pub fn span_with_args(
        &self,
        name: impl Into<String>,
        args: BTreeMap<String, serde_json::Value>,
    ) -> GutchProfileSpan {
        let name = name.into();
        let process_id = self.state.process_id;
        let thread_id = self.thread_id();
        let started_at = Instant::now();
        self.record_event(TraceEvent {
            name: name.clone(),
            cat: TRACE_CATEGORY,
            ph: "B",
            ts: self.timestamp(started_at),
            pid: process_id,
            tid: thread_id,
            args,
        });
        GutchProfileSpan {
            profiler: Some(self.clone()),
            name,
            started_at,
            process_id,
            thread_id,
        }
    }

    pub fn event_with_args(
        &self,
        name: impl Into<String>,
        args: BTreeMap<String, serde_json::Value>,
    ) {
        self.record_event(TraceEvent {
            name: name.into(),
            cat: TRACE_CATEGORY,
            ph: "i",
            ts: self.timestamp(Instant::now()),
            pid: self.state.process_id,
            tid: self.thread_id(),
            args,
        });
    }

    pub fn summary(&self) -> GutchProfileSummary {
        let aggregates = self
            .state
            .aggregates
            .lock()
            .expect("Gutch profiler aggregate lock poisoned");
        let mut spans = aggregates
            .iter()
            .map(|(name, aggregate)| GutchProfileSpanSummary {
                name: name.clone(),
                count: aggregate.count,
                total_seconds: aggregate.total.as_secs_f64(),
                maximum_seconds: aggregate.maximum.as_secs_f64(),
            })
            .collect::<Vec<_>>();
        spans.sort_by(|left, right| {
            right
                .total_seconds
                .total_cmp(&left.total_seconds)
                .then_with(|| left.name.cmp(&right.name))
        });
        GutchProfileSummary {
            span_count: spans.iter().map(|span| span.count).sum(),
            total_span_seconds: spans.iter().map(|span| span.total_seconds).sum(),
            spans,
        }
    }

    pub fn write_to(&self, path: &Path) -> crate::Result<()> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        let mut events = self
            .state
            .events
            .lock()
            .expect("Gutch profiler event lock poisoned")
            .clone();
        let process_name = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .filter(|stem| !stem.is_empty())
            .unwrap_or("gutch");
        for event in &mut events {
            if event.cat == "__metadata" && event.name == "process_name" {
                event
                    .args
                    .insert(String::from("name"), serde_json::json!(process_name));
            }
        }
        events.sort_by_key(|event| event.ts);
        let document = TraceDocument {
            trace_events: events,
            display_time_unit: "ms",
        };
        fs::write(path, serde_json::to_vec_pretty(&document)?)?;
        Ok(())
    }

    fn metadata_event(&self, name: &str, args: BTreeMap<String, serde_json::Value>) {
        self.record_event(TraceEvent {
            name: name.to_string(),
            cat: "__metadata",
            ph: "M",
            ts: 0,
            pid: self.state.process_id,
            tid: self.thread_id(),
            args,
        });
    }

    fn timestamp(&self, instant: Instant) -> u64 {
        instant
            .saturating_duration_since(self.state.started_at)
            .as_micros()
            .min(u64::MAX as u128) as u64
    }

    fn record_event(&self, event: TraceEvent) {
        self.state
            .events
            .lock()
            .expect("Gutch profiler event lock poisoned")
            .push(event);
    }

    fn thread_id(&self) -> u64 {
        let current_thread = std::thread::current().id();
        let mut thread_ids = self
            .state
            .thread_ids
            .lock()
            .expect("Gutch profiler thread ID lock poisoned");
        if let Some(thread_id) = thread_ids.get(&current_thread) {
            return *thread_id;
        }
        let thread_id = thread_ids.len() as u64 + 1;
        thread_ids.insert(current_thread, thread_id);
        thread_id
    }

    fn finish_span(&self, name: String, started_at: Instant, process_id: u32, thread_id: u64) {
        let duration = started_at.elapsed();
        self.record_event(TraceEvent {
            name: name.clone(),
            cat: TRACE_CATEGORY,
            ph: "E",
            ts: self.timestamp(Instant::now()),
            pid: process_id,
            tid: thread_id,
            args: BTreeMap::new(),
        });
        let mut aggregates = self
            .state
            .aggregates
            .lock()
            .expect("Gutch profiler aggregate lock poisoned");
        let aggregate = aggregates.entry(name).or_default();
        aggregate.count += 1;
        aggregate.total += duration;
        aggregate.maximum = aggregate.maximum.max(duration);
    }
}

impl Default for GutchProfiler {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for GutchProfileSpan {
    fn drop(&mut self) {
        let Some(profiler) = self.profiler.take() else {
            return;
        };
        profiler.finish_span(
            std::mem::take(&mut self.name),
            self.started_at,
            self.process_id,
            self.thread_id,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::GutchProfiler;
    use super::file_trace_args;

    #[test]
    fn file_trace_args_use_mib_and_relative_path() {
        let args = file_trace_args("bin/game.exe", 3 * 1024 * 1024 + 512 * 1024);

        assert_eq!(args["path"], serde_json::json!("bin/game.exe"));
        assert_eq!(args["size_mib"], serde_json::json!(3.5));
    }

    #[test]
    fn trace_process_name_uses_output_file_stem() {
        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        let trace_path = temporary_directory.path().join("local-reconstruct.json");
        let profiler = GutchProfiler::new();
        let span = profiler.span("test");
        drop(span);

        profiler.write_to(&trace_path).expect("write trace");

        let document: serde_json::Value =
            serde_json::from_slice(&std::fs::read(trace_path).expect("read trace"))
                .expect("parse trace");
        let process_name = document["traceEvents"]
            .as_array()
            .expect("trace events")
            .iter()
            .find(|event| event["name"] == "process_name")
            .and_then(|event| event["args"]["name"].as_str());
        assert_eq!(process_name, Some("local-reconstruct"));
    }
}

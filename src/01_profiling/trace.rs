//! Compact trace arguments and Chrome/Perfetto JSON trace export.

use serde::Serialize;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::atomic::Ordering;

use super::{ARGUMENT_FIELDS, Profiler};

#[derive(Debug)]
pub(super) struct TraceText<const N: usize> {
    bytes: [u8; N],
    len: usize,
}
impl<const N: usize> TraceText<N> {
    pub(super) fn new(value: &str) -> Self {
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
pub(super) enum TraceValue {
    Unsigned(u64),
    Signed(i64),
    Float(f64),
    Boolean(bool),
    Text(TraceText<192>),
}
#[derive(Debug)]
pub(super) struct TraceField {
    pub(super) key: TraceText<48>,
    pub(super) value: TraceValue,
}
#[derive(Debug)]
pub(super) struct TraceArguments(pub(super) [Option<TraceField>; ARGUMENT_FIELDS]);
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

impl Profiler {
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

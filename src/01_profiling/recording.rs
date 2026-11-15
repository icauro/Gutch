//! Per-thread recorders and the event, byte and argument budgets that bound
//! a recording.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, Weak};
use std::time::Instant;

use super::trace::{TraceArguments, TraceField, TraceText, TraceValue};
use super::{
    ARGUMENT_FIELDS, Arguments, BLOCK_EVENTS, Event, OVERVIEW_BYTES, Profiler, RECORDING_BYTES,
    Recorder, overview_marker,
};
use std::array::from_fn;
use std::thread;

thread_local! {
    static RECORDERS: RefCell<HashMap<u64, Weak<Mutex<Recorder>>>> = RefCell::new(HashMap::new());
}

impl Profiler {
    pub(super) fn recorder(&self) -> Option<Arc<Mutex<Recorder>>> {
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
                thread_name: thread::current()
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
    pub(super) fn admit(&self) -> bool {
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
    pub(super) fn reserve_event(
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
        let mut fields = from_fn(|_| None);
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
}

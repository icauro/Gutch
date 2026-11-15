//! Coalesced operation feedback, independent of diagnostic profiling.
use super::{ProgressEvent, ProgressSink};
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

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
    fn snapshot(&mut self, bytes: u64, now: Instant) -> Option<Throughput> {
        let start = self.start?;
        let end = self.end.unwrap_or(now);
        if self.samples.back().is_none_or(|(at, _)| *at < end) {
            self.samples.push_back((end, bytes));
        }
        loop {
            let exceeds_sample_limit = self.samples.len() > 10;
            // Retain two samples even after a pause so a recent rate can be computed.
            let oldest_sample_outside_window = self.samples.len() > 2
                && end.saturating_duration_since(self.samples[1].0) > Duration::from_secs(2);
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
#[derive(Debug, Default)]
struct FeedbackState {
    event: Option<ProgressEvent>,
    revision: u64,
    stop: bool,
    download: ThroughputHistory,
    output: ThroughputHistory,
    profiler: Option<Arc<crate::profiling::Profiler>>,
}
#[derive(Debug, Default)]
struct SharedFeedbackState {
    state: Mutex<FeedbackState>,
    changed: Condvar,
    downloaded: AtomicU64,
    written: AtomicU64,
}
#[derive(Debug)]
pub(crate) struct LiveProgress {
    shared: Arc<SharedFeedbackState>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}
impl LiveProgress {
    pub(crate) fn new(sink: Arc<dyn ProgressSink>) -> Arc<Self> {
        let shared = Arc::new(SharedFeedbackState::default());
        let thread_shared = shared.clone();
        let thread = std::thread::Builder::new()
            .name("gutch-progress".into())
            .spawn(move || {
                let mut previous_revision = 0;
                loop {
                    let (event, stop) = {
                        let Ok(state) = thread_shared.state.lock() else {
                            return;
                        };
                        let mut state = if state.stop {
                            state
                        } else {
                            let Ok((state, _)) = thread_shared
                                .changed
                                .wait_timeout(state, Duration::from_millis(250))
                            else {
                                return;
                            };
                            state
                        };
                        let now = Instant::now();
                        let feedback = LiveFeedback {
                            download: state
                                .download
                                .snapshot(thread_shared.downloaded.load(Ordering::Relaxed), now),
                            output_write: state
                                .output
                                .snapshot(thread_shared.written.load(Ordering::Relaxed), now),
                        };
                        let active = feedback.download.as_ref().is_some_and(|s| !s.finished)
                            || feedback.output_write.as_ref().is_some_and(|s| !s.finished);
                        let event = if state.revision != previous_revision || active || state.stop {
                            state.event.clone().map(|mut event| {
                                event.feedback = Some(feedback);
                                event
                            })
                        } else {
                            None
                        };
                        previous_revision = state.revision;
                        (event, state.stop)
                    };
                    if let Some(event) = event {
                        let profiler = thread_shared
                            .state
                            .lock()
                            .ok()
                            .and_then(|s| s.profiler.clone());
                        if let Some(profiler) = profiler {
                            profiler.event_with_args("operation.progress", || {
                                let mut args = std::collections::BTreeMap::new();
                                if let Some(feedback) = &event.feedback {
                                    for (prefix, rate) in [
                                        ("download", &feedback.download),
                                        ("output_write", &feedback.output_write),
                                    ] {
                                        if let Some(rate) = rate {
                                            args.insert(
                                                format!("{prefix}_bytes"),
                                                serde_json::json!(rate.bytes),
                                            );
                                            if let Some(recent) = rate.recent_bytes_per_second {
                                                args.insert(
                                                    format!("{prefix}_bytes_per_second"),
                                                    serde_json::json!(recent),
                                                );
                                            }
                                        }
                                    }
                                }
                                args
                            });
                        }
                        sink.emit(event);
                    }
                    if stop {
                        break;
                    }
                }
            })
            .ok();
        Arc::new(Self {
            shared,
            thread: Mutex::new(thread),
        })
    }
    pub(crate) fn with_profiler(&self, profiler: Arc<crate::profiling::Profiler>) {
        if let Ok(mut state) = self.shared.state.lock() {
            state.profiler = Some(profiler);
        }
    }
    pub(crate) fn start(&self, download: bool) {
        if let Ok(mut state) = self.shared.state.lock() {
            let rate = if download {
                &mut state.download
            } else {
                &mut state.output
            };
            if rate.start.is_none() {
                let now = Instant::now();
                rate.start = Some(now);
                rate.samples.push_back((now, 0));
            }
        }
    }
    pub(crate) fn finish(&self, download: bool) {
        if let Ok(mut state) = self.shared.state.lock() {
            let rate = if download {
                &mut state.download
            } else {
                &mut state.output
            };
            if rate.start.is_some() && rate.end.is_none() {
                rate.end = Some(Instant::now());
            }
            state.revision += 1;
        }
    }
    pub(crate) fn record(&self, download: bool, bytes: u64) {
        (if download {
            &self.shared.downloaded
        } else {
            &self.shared.written
        })
        .fetch_add(bytes, Ordering::Relaxed);
    }
}
impl ProgressSink for LiveProgress {
    fn emit(&self, event: ProgressEvent) {
        if let Ok(mut state) = self.shared.state.lock() {
            let transition = state
                .event
                .as_ref()
                .is_none_or(|last| last.phase != event.phase || last.done != event.done);
            state.event = Some(event);
            state.revision += 1;
            if transition {
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
            if state.download.start.is_some() {
                state.download.end.get_or_insert(now);
            }
            if state.output.start.is_some() {
                state.output.end.get_or_insert(now);
            }
        }
        self.shared.changed.notify_one();
        if let Ok(mut thread) = self.thread.lock()
            && let Some(thread) = thread.take()
        {
            let _ = thread.join();
        }
    }
}
pub(crate) struct OutputStage(Option<Arc<LiveProgress>>);
impl OutputStage {
    pub(crate) fn new(feedback: Option<Arc<LiveProgress>>) -> Self {
        if let Some(f) = &feedback {
            f.start(false);
        }
        Self(feedback)
    }
}
impl Drop for OutputStage {
    fn drop(&mut self) {
        if let Some(f) = &self.0 {
            f.finish(false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Debug)]
    struct PausedSink {
        entered: std::sync::mpsc::Sender<()>,
        resume: Mutex<std::sync::mpsc::Receiver<()>>,
        paused: std::sync::atomic::AtomicBool,
        events: Mutex<Vec<ProgressEvent>>,
    }
    impl ProgressSink for PausedSink {
        fn emit(&self, event: ProgressEvent) {
            if !self.paused.swap(true, Ordering::Relaxed) {
                self.entered.send(()).unwrap();
                self.resume.lock().unwrap().recv().unwrap();
            }
            self.events.lock().unwrap().push(event);
        }
    }
    #[test]
    fn slow_consumer_coalesces_and_receives_final_concurrent_rates() {
        let (entered, waiting) = std::sync::mpsc::channel();
        let (resume, paused) = std::sync::mpsc::channel();
        let sink = Arc::new(PausedSink {
            entered,
            resume: Mutex::new(paused),
            paused: std::sync::atomic::AtomicBool::new(false),
            events: Mutex::new(Vec::new()),
        });
        let feedback = LiveProgress::new(sink.clone());
        feedback.start(true);
        feedback.start(false);
        let mut event = ProgressEvent {
            feedback: None,
            operation: super::super::OperationKind::Fetch,
            phase: super::super::ProgressPhase::Reconstruct,
            message: "work".into(),
            current: 0,
            total: 10000,
            transferred_bytes: 0,
            total_bytes: 0,
            detail: None,
            done: false,
        };
        feedback.emit(event.clone());
        waiting.recv_timeout(Duration::from_secs(2)).unwrap();
        // A blocked consumer cannot block these updates or enqueue 10,000 snapshots.
        for current in 1..=10000 {
            feedback.record(true, 3);
            feedback.record(false, 2);
            event.current = current;
            feedback.emit(event.clone());
        }
        feedback.finish(true);
        feedback.finish(false);
        resume.send(()).unwrap();
        drop(feedback);
        let events = sink.events.lock().unwrap();
        assert!(events.len() <= 3);
        let event = events.last().unwrap();
        assert_eq!(event.current, 10000);
        let rates = event.feedback.as_ref().unwrap();
        assert_eq!(rates.download.as_ref().unwrap().bytes, 30000);
        assert_eq!(rates.output_write.as_ref().unwrap().bytes, 20000);
        assert!(rates.download.as_ref().unwrap().finished);
        assert!(rates.output_write.as_ref().unwrap().finished);
    }
    #[test]
    fn recent_rate_includes_stalls_and_average_freezes() {
        let start = Instant::now();
        let mut rate = ThroughputHistory {
            start: Some(start),
            ..Default::default()
        };
        rate.samples.push_back((start, 0));
        assert_eq!(
            rate.snapshot(200, start + Duration::from_secs(1))
                .unwrap()
                .average_bytes_per_second,
            Some(200.0)
        );
        for n in 2..=5 {
            rate.snapshot(200, start + Duration::from_secs(n));
        }
        assert_eq!(
            rate.snapshot(200, start + Duration::from_secs(6))
                .unwrap()
                .recent_bytes_per_second,
            Some(0.0)
        );
        rate.end = Some(start + Duration::from_secs(2));
        assert_eq!(
            rate.snapshot(200, start + Duration::from_secs(8))
                .unwrap()
                .average_bytes_per_second,
            Some(100.0)
        );
    }
}

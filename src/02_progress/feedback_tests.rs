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
    feedback.start(Stream::Download);
    feedback.start(Stream::OutputWrite);
    let mut event = ProgressEvent::new(
        super::super::OperationKind::Fetch,
        super::super::ProgressPhase::Reconstruct,
        "work",
        0,
        10000,
    );
    feedback.emit(event.clone());
    waiting.recv_timeout(Duration::from_secs(2)).unwrap();
    // A blocked consumer cannot block these updates or enqueue 10,000 snapshots.
    for current in 1..=10000 {
        feedback.record(Stream::Download, 3);
        feedback.record(Stream::OutputWrite, 2);
        event.current = current;
        feedback.emit(event.clone());
    }
    feedback.finish(Stream::Download);
    feedback.finish(Stream::OutputWrite);
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
fn the_final_done_event_is_delivered_once() {
    let sink = Arc::new(crate::test_util::RecordedProgress::default());
    let feedback = LiveProgress::new(sink.clone());
    feedback.emit(ProgressEvent::finished(
        super::super::OperationKind::Fetch,
        "done",
        1,
        1,
    ));
    // Let the thread deliver the transition before the drop stops it.
    let deadline = Instant::now() + Duration::from_secs(5);
    while sink.events().is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    drop(feedback);
    let events = sink.events();
    assert_eq!(events.iter().filter(|event| event.done).count(), 1);
}

#[test]
fn recent_rate_includes_stalls_and_average_freezes() {
    let start = Instant::now();
    let mut rate = ThroughputHistory::default();
    rate.start(start);
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
    assert!(rate.finish(start + Duration::from_secs(2)));
    assert_eq!(
        rate.snapshot(200, start + Duration::from_secs(8))
            .unwrap()
            .average_bytes_per_second,
        Some(100.0)
    );
}

use super::*;
#[test]
fn nested_trace_endpoints_remain_inside_the_parent() {
    let profiler = Profiler::new(ProfileMode::Detailed);
    for _ in 0..1024 {
        let _parent = profiler.span("test.parent");
        let _child = profiler.span("test.child");
    }
    let recorders = profiler.state.recorders.lock().unwrap();
    let recorder = recorders[0].lock().unwrap();
    for pair in recorder.events.chunks_exact(2) {
        let (parent, child) = (&pair[0], &pair[1]);
        assert!(parent.ts <= child.ts);
        assert!(child.ts + child.duration.unwrap() <= parent.ts + parent.duration.unwrap());
    }
}

#[test]
fn disabled_and_summary_do_not_evaluate_arguments() {
    for mode in [ProfileMode::Off, ProfileMode::Summary] {
        let p = Profiler::new(mode);
        drop(p.span_with_args("test", || panic!("arguments evaluated")));
        p.event_with_args("event", || panic!("arguments evaluated"));
    }
}
#[test]
fn millions_of_job_markers_do_not_build_millions_of_arguments() {
    let p = Profiler::new(ProfileMode::Detailed);
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
    let p = Profiler::new(ProfileMode::Detailed);
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
    let trace: serde_json::Value =
        serde_json::from_slice(&std::fs::read(directory.path().join("trace.json")).unwrap())
            .unwrap();
    assert!(
        trace["traceEvents"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| {
                event["name"] == "profile.recording_incomplete"
                    && event["args"]["dropped_events"].as_u64().unwrap() > 0
            })
    );
}
#[test]
fn consecutive_jobs_are_recorded_without_time_sampling() {
    let profiler = Profiler::new(ProfileMode::Detailed);
    for _ in 0..512 {
        drop(profiler.span("capture.scan.file"));
        drop(profiler.span("capture.bundle.encode_and_write"));
    }
    let summary = profiler.summary();
    assert_eq!(summary.spans.len(), 2);
    assert!(
        summary
            .spans
            .iter()
            .all(|span| span.count == 512 && !span.sampled)
    );
    assert_eq!(summary.suppressed_events, 0);
    assert_eq!(summary.dropped_events, 0);
    let recorder = profiler.recorder().unwrap();
    let recorder = recorder.lock().unwrap();
    assert_eq!(recorder.events.len(), 1024);
    assert!(recorder.events.iter().all(|event| event.duration.is_some()));
    assert!(ProfileInstant::start(None).0.is_none());
    let summary = Profiler::new(ProfileMode::Summary);
    assert!(ProfileInstant::detailed(Some(&summary)).0.is_none());
}
#[test]
fn finalization_rejects_live_spans_and_reports_export_failure() {
    let profiler = Profiler::new(ProfileMode::Detailed);
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
    let profiler = Profiler::new(ProfileMode::Detailed);
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
    let a = Profiler::new(ProfileMode::Detailed);
    let b = Profiler::new(ProfileMode::Summary);
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

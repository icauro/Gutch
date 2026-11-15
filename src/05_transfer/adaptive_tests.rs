use super::*;

fn observe_rate(state: &mut AdaptiveTransferState, rate: u64) -> Option<usize> {
    state.observe(TransferObservation {
        now: state.previous_sample_at + ADAPTIVE_SAMPLE_INTERVAL,
        transferred_bytes: state.previous_bytes + rate * 2,
        retry_count: 0,
        downstream_backpressure: false,
        has_queued_work: true,
        active_workers: state.target,
    })
}

#[test]
fn adaptive_stalls_never_ramp_and_abort_an_unproductive_probe() {
    let mut state = AdaptiveTransferState::new(4, 16);
    for _ in 0..30 {
        assert_eq!(observe_rate(&mut state, 0), None);
        assert_eq!(state.target, 4);
    }
    for _ in 0..4 {
        observe_rate(&mut state, 100);
    }
    assert_eq!(state.target, 5);
    assert_eq!(observe_rate(&mut state, 0), Some(4));
    for _ in 0..30 {
        assert_eq!(observe_rate(&mut state, 0), None);
    }
}

#[test]
fn adaptive_probes_constant_throughput_then_rolls_back_with_cooldown() {
    let mut state = AdaptiveTransferState::new(4, 16);
    assert_eq!(observe_rate(&mut state, 100), None);
    assert_eq!(observe_rate(&mut state, 100), Some(5));
    assert_eq!(observe_rate(&mut state, 100), None);
    assert_eq!(observe_rate(&mut state, 100), Some(4));
    for _ in 0..ADAPTIVE_COOLDOWN_WINDOWS {
        assert_eq!(observe_rate(&mut state, 100), None);
    }
    for _ in 0..60 {
        observe_rate(&mut state, 100);
        assert!((4..=5).contains(&state.target));
    }
}

#[test]
fn adaptive_finds_per_connection_capacity_without_running_away_at_saturation() {
    let mut state = AdaptiveTransferState::new(4, 16);
    let mut peak = 4;
    for _ in 0..60 {
        let rate = (state.target as u64 * 100).min(800);
        observe_rate(&mut state, rate);
        peak = peak.max(state.target);
    }
    assert_eq!(peak, 9);
    assert!((8..=9).contains(&state.target));
    let mut capped = AdaptiveTransferState::new(4, 6);
    for _ in 0..60 {
        let rate = capped.target as u64 * 100;
        observe_rate(&mut capped, rate);
        assert!(capped.target <= 6);
    }
    assert_eq!(capped.target, 6);
}

#[test]
fn adaptive_averages_regressions_and_ignores_bursty_growth() {
    let mut state = AdaptiveTransferState::new(4, 4);
    observe_rate(&mut state, 100);
    observe_rate(&mut state, 100);
    assert_eq!(observe_rate(&mut state, 20), None);
    assert_eq!(observe_rate(&mut state, 20), Some(3));
    let mut bursty = AdaptiveTransferState::new(4, 16);
    for _ in 0..30 {
        observe_rate(&mut bursty, 100);
        observe_rate(&mut bursty, 300);
        assert_eq!(bursty.target, 4);
    }
}

#[test]
fn adaptive_retries_and_backpressure_reduce_even_during_cooldown() {
    let mut state = AdaptiveTransferState::new(4, 16);
    for (retries, pressure, expected) in [(1, false, 3), (1, true, 2), (2, false, 1)] {
        assert_eq!(
            state.observe(TransferObservation {
                now: state.previous_sample_at + ADAPTIVE_SAMPLE_INTERVAL,
                transferred_bytes: state.previous_bytes + 100,
                retry_count: retries,
                downstream_backpressure: pressure,
                has_queued_work: true,
                active_workers: state.target,
            }),
            Some(expected)
        );
    }
    assert_eq!(
        state.observe(TransferObservation {
            now: state.previous_sample_at + ADAPTIVE_SAMPLE_INTERVAL,
            transferred_bytes: state.previous_bytes + 100,
            retry_count: 3,
            downstream_backpressure: true,
            has_queued_work: true,
            active_workers: state.target,
        }),
        None
    );
    assert_eq!(state.target, 1);
}

#[test]
fn adaptive_does_not_probe_a_draining_queue_or_short_observation() {
    let mut state = AdaptiveTransferState::new(4, 16);
    assert_eq!(
        state.observe(TransferObservation {
            now: state.previous_sample_at + Duration::from_secs(1),
            transferred_bytes: 100,
            retry_count: 0,
            downstream_backpressure: false,
            has_queued_work: true,
            active_workers: 4,
        }),
        None
    );
    for _ in 0..30 {
        assert_eq!(
            state.observe(TransferObservation {
                now: state.previous_sample_at + ADAPTIVE_SAMPLE_INTERVAL,
                transferred_bytes: state.previous_bytes + 100,
                retry_count: 0,
                downstream_backpressure: false,
                has_queued_work: false,
                active_workers: 4,
            }),
            None
        );
    }
    assert_eq!(state.target, 4);
}

#[test]
fn adaptive_waits_for_actual_worker_count_before_evaluating_a_probe() {
    let mut state = AdaptiveTransferState::new(4, 16);
    observe_rate(&mut state, 100);
    observe_rate(&mut state, 100);
    assert_eq!(state.target, 5);
    for _ in 0..10 {
        assert_eq!(
            state.observe(TransferObservation {
                now: state.previous_sample_at + ADAPTIVE_SAMPLE_INTERVAL,
                transferred_bytes: state.previous_bytes + 200,
                retry_count: 0,
                downstream_backpressure: false,
                has_queued_work: true,
                active_workers: 4,
            }),
            None
        );
    }
    assert_eq!(observe_rate(&mut state, 100), None);
    assert_eq!(observe_rate(&mut state, 100), Some(4));
}

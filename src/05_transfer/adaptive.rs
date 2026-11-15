//! Pure adaptive concurrency decisions, separate from worker lifecycle.
use std::time::{Duration, Instant};

pub(super) const ADAPTIVE_SAMPLE_INTERVAL: Duration = Duration::from_secs(2);
pub(super) const ADAPTIVE_MIN_IMPROVEMENT: f64 = 1.05;
pub(super) const ADAPTIVE_MAX_REGRESSION: f64 = 0.90;
pub(super) const ADAPTIVE_STABLE_WINDOWS: usize = 2;
pub(super) const ADAPTIVE_COOLDOWN_WINDOWS: usize = 2;

pub(super) struct TransferObservation {
    pub(super) now: Instant,
    pub(super) transferred_bytes: u64,
    pub(super) retry_count: usize,
    pub(super) downstream_backpressure: bool,
    pub(super) has_queued_work: bool,
    pub(super) active_workers: usize,
}

pub(super) struct AdaptiveTransferState {
    target: usize,
    maximum: usize,
    previous_sample_at: Instant,
    pub(super) previous_bytes: u64,
    previous_rate: Option<f64>,
    probe_baseline: Option<f64>,
    cooldown_windows: usize,
    window_count: usize,
    window_bytes: u64,
    window_seconds: f64,
    minimum_rate: f64,
    maximum_rate: f64,
    last_retry_count: usize,
}

impl AdaptiveTransferState {
    pub(super) fn new(initial: usize, maximum: usize) -> Self {
        Self {
            target: initial,
            maximum,
            previous_sample_at: Instant::now(),
            previous_bytes: 0,
            previous_rate: None,
            probe_baseline: None,
            cooldown_windows: 0,
            window_count: 0,
            window_bytes: 0,
            window_seconds: 0.0,
            minimum_rate: f64::INFINITY,
            maximum_rate: 0.0,
            last_retry_count: 0,
        }
    }

    pub(super) fn observe(&mut self, observation: TransferObservation) -> Option<usize> {
        let TransferObservation {
            now,
            transferred_bytes,
            retry_count,
            downstream_backpressure,
            has_queued_work,
            active_workers,
        } = observation;
        let elapsed = now.saturating_duration_since(self.previous_sample_at);
        if elapsed < ADAPTIVE_SAMPLE_INTERVAL {
            return None;
        }
        let bytes = transferred_bytes.saturating_sub(self.previous_bytes);
        let seconds = elapsed.as_secs_f64();
        let rate = bytes as f64 / seconds;
        let had_new_retry = retry_count > self.last_retry_count;
        self.previous_sample_at = now;
        self.previous_bytes = transferred_bytes;
        self.last_retry_count = retry_count;

        if had_new_retry || downstream_backpressure {
            self.back_off();
            return self.reduce_target();
        }
        if !has_queued_work {
            // A draining queue cannot use another worker or measure a probe fairly.
            self.reset_window();
            self.previous_rate = None;
            self.probe_baseline = None;
            return None;
        }
        if bytes == 0 {
            // Zero compared with zero is not an improvement. Undo an active probe
            // or back off once after progress stops, then wait for positive evidence.
            let was_active = self.previous_rate.is_some() || self.probe_baseline.is_some();
            self.back_off();
            return if was_active {
                self.reduce_target()
            } else {
                None
            };
        }
        if self.cooldown_windows > 0 {
            self.cooldown_windows -= 1;
            return None;
        }
        if active_workers != self.target {
            // Lowering the target never cancels healthy downloads. Wait until
            // the changed concurrency is actually in effect before comparing it.
            self.reset_window();
            return None;
        }
        self.window_count += 1;
        self.window_bytes = self.window_bytes.saturating_add(bytes);
        self.window_seconds += seconds;
        self.minimum_rate = self.minimum_rate.min(rate);
        self.maximum_rate = self.maximum_rate.max(rate);
        if self.window_count < ADAPTIVE_STABLE_WINDOWS {
            return None;
        }
        let average_rate = self.window_bytes as f64 / self.window_seconds;
        let stable = self.minimum_rate >= self.maximum_rate * ADAPTIVE_MAX_REGRESSION;
        self.reset_window();

        if let Some(baseline) = self.probe_baseline.take() {
            // Compare averaged work after the change with work before it. A flat
            // rate at the new concurrency means the extra worker did not help.
            if average_rate < baseline * ADAPTIVE_MIN_IMPROVEMENT {
                self.back_off();
                return self.reduce_target();
            }
        } else if self
            .previous_rate
            .is_some_and(|previous| average_rate < previous * ADAPTIVE_MAX_REGRESSION)
        {
            self.back_off();
            return self.reduce_target();
        }
        self.previous_rate = Some(average_rate);
        if stable && self.target < self.maximum {
            self.probe_baseline = Some(average_rate);
            return self.increase_target();
        }
        None
    }

    fn reset_window(&mut self) {
        self.window_count = 0;
        self.window_bytes = 0;
        self.window_seconds = 0.0;
        self.minimum_rate = f64::INFINITY;
        self.maximum_rate = 0.0;
    }

    fn back_off(&mut self) {
        self.reset_window();
        self.previous_rate = None;
        self.probe_baseline = None;
        self.cooldown_windows = ADAPTIVE_COOLDOWN_WINDOWS;
    }

    fn increase_target(&mut self) -> Option<usize> {
        if self.target >= self.maximum {
            return None;
        }
        self.target += 1;
        Some(self.target)
    }

    fn reduce_target(&mut self) -> Option<usize> {
        if self.target <= 1 {
            return None;
        }
        self.target -= 1;
        Some(self.target)
    }
}

#[cfg(test)]
mod tests {
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
}

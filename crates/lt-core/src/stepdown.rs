//! Automatic step-down from Continuous to Light when the PC is busy or the final text falls
//! behind. Pure: the scheduler feeds it one
//! sample per second and applies the mode it reports.

use crate::{
    config::LatencyConfig,
    types::{EffectiveMode, ModeReason},
};

const DOWN_CPU_SAMPLES: u32 = 3;
const DOWN_LAG_SAMPLES: u32 = 2;
const UP_CPU_SAMPLES: u32 = 10;
const UP_LAG_S: f64 = 1.5;
const MIN_SECONDS_BETWEEN_SWITCHES: f64 = 10.0;
const LOCK_AFTER_STEP_DOWNS: usize = 3;
const LOCK_WINDOW_S: f64 = 300.0;
const DRAFT_FAILURES_BEFORE_LIGHT: u32 = 3;

/// The effective mode and why it differs from the chosen one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModeState {
    pub mode: EffectiveMode,
    pub reason: Option<ModeReason>,
}

pub struct StepDown {
    enabled: bool,
    cpu_down: f32,
    cpu_up: f32,
    lag_down_s: f64,
    /// What the user or `auto` chose for this session: Continuous, Light or Off.
    chosen: EffectiveMode,
    base_reason: Option<ModeReason>,
    stepped: Option<ModeReason>,
    draft_unavailable: bool,
    locked: bool,
    cpu_over: u32,
    lag_over: u32,
    cpu_under: u32,
    last_switch_s: Option<f64>,
    step_downs: Vec<f64>,
    draft_failures: u32,
    current: ModeState,
}

impl StepDown {
    pub fn new(config: &LatencyConfig, chosen: EffectiveMode, reason: Option<ModeReason>) -> Self {
        Self {
            enabled: config.step_down,
            cpu_down: config.step_down_cpu_pct,
            cpu_up: config.step_up_cpu_pct,
            lag_down_s: f64::from(config.step_down_lag_s),
            chosen,
            base_reason: reason,
            stepped: None,
            draft_unavailable: false,
            locked: false,
            cpu_over: 0,
            lag_over: 0,
            cpu_under: 0,
            last_switch_s: None,
            step_downs: Vec::new(),
            draft_failures: 0,
            current: ModeState {
                mode: chosen,
                reason,
            },
        }
    }

    pub fn state(&self) -> ModeState {
        self.current
    }

    pub fn is_locked(&self) -> bool {
        self.locked
    }

    /// The user picked Continuous or Light while listening (Off restarts the pipeline).
    pub fn choose(&mut self, mode: EffectiveMode, now_s: f64) -> Option<ModeState> {
        self.chosen = mode;
        self.base_reason = Some(ModeReason::User);
        self.stepped = None;
        self.locked = false;
        self.step_downs.clear();
        self.cpu_over = 0;
        self.lag_over = 0;
        self.cpu_under = 0;
        self.last_switch_s = Some(now_s);
        self.recompute()
    }

    fn recompute(&mut self) -> Option<ModeState> {
        let next = if self.chosen != EffectiveMode::Continuous {
            ModeState {
                mode: self.chosen,
                reason: self.base_reason,
            }
        } else if self.draft_unavailable {
            ModeState {
                mode: EffectiveMode::Light,
                reason: Some(ModeReason::DraftUnavailable),
            }
        } else if let Some(reason) = self.stepped {
            ModeState {
                mode: EffectiveMode::Light,
                reason: Some(reason),
            }
        } else {
            ModeState {
                mode: EffectiveMode::Continuous,
                reason: self.base_reason,
            }
        };
        (next != self.current).then(|| {
            self.current = next;
            next
        })
    }

    /// One sample per second while listening. `lag_s` is how far the oldest unfinished final
    /// translation trails the stream. Returns the new state when it changed.
    pub fn sample(&mut self, now_s: f64, cpu_pct: f32, lag_s: f64) -> Option<ModeState> {
        if !self.enabled || self.chosen != EffectiveMode::Continuous {
            return None;
        }
        if self.current.mode == EffectiveMode::Continuous {
            self.cpu_over = if cpu_pct >= self.cpu_down {
                self.cpu_over + 1
            } else {
                0
            };
            self.lag_over = if lag_s >= self.lag_down_s {
                self.lag_over + 1
            } else {
                0
            };
            let reason = if self.cpu_over >= DOWN_CPU_SAMPLES {
                Some(ModeReason::Cpu)
            } else if self.lag_over >= DOWN_LAG_SAMPLES {
                Some(ModeReason::Lag)
            } else {
                None
            };
            if let Some(reason) = reason {
                self.cpu_over = 0;
                self.lag_over = 0;
                self.cpu_under = 0;
                self.stepped = Some(reason);
                self.last_switch_s = Some(now_s);
                self.step_downs.push(now_s);
                self.step_downs.retain(|at| now_s - at <= LOCK_WINDOW_S);
                if self.step_downs.len() >= LOCK_AFTER_STEP_DOWNS {
                    self.locked = true;
                    tracing::info!("stepped down three times in five minutes; staying in Light");
                }
                return self.recompute();
            }
            return None;
        }
        // Light after an automatic step-down: look for room to return.
        if self.stepped.is_none() || self.locked || self.draft_unavailable {
            return None;
        }
        self.cpu_under = if cpu_pct < self.cpu_up {
            self.cpu_under + 1
        } else {
            0
        };
        let settled = self
            .last_switch_s
            .is_none_or(|at| now_s - at >= MIN_SECONDS_BETWEEN_SWITCHES);
        if self.cpu_under >= UP_CPU_SAMPLES && lag_s < UP_LAG_S && settled {
            self.stepped = None;
            self.cpu_under = 0;
            self.last_switch_s = Some(now_s);
            return self.recompute();
        }
        None
    }

    /// A draft request failed or timed out.
    pub fn draft_failed(&mut self) -> Option<ModeState> {
        self.draft_failures += 1;
        if self.draft_failures >= DRAFT_FAILURES_BEFORE_LIGHT {
            self.draft_unavailable = true;
            return self.recompute();
        }
        None
    }

    pub fn draft_succeeded(&mut self) {
        self.draft_failures = 0;
    }

    /// The draft server's state changed. A failed server means no drafts until it is ready.
    pub fn draft_server(&mut self, ready: bool) -> Option<ModeState> {
        if ready {
            self.draft_failures = 0;
        }
        self.draft_unavailable = !ready;
        self.recompute()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn controller() -> StepDown {
        StepDown::new(
            &LatencyConfig::default(),
            EffectiveMode::Continuous,
            Some(ModeReason::Auto),
        )
    }

    fn down(controller: &mut StepDown, start: f64) -> ModeState {
        let mut result = None;
        for second in 0..3 {
            result = controller.sample(start + f64::from(second), 85.0, 0.0);
        }
        result.expect("three busy samples step down")
    }

    #[test]
    fn three_busy_cpu_samples_step_down() {
        let mut c = controller();
        assert_eq!(c.sample(0.0, 90.0, 0.0), None);
        assert_eq!(c.sample(1.0, 90.0, 0.0), None);
        // a dip resets the count
        assert_eq!(c.sample(2.0, 50.0, 0.0), None);
        assert_eq!(c.sample(3.0, 90.0, 0.0), None);
        assert_eq!(c.sample(4.0, 90.0, 0.0), None);
        let state = c.sample(5.0, 80.0, 0.0).unwrap();
        assert_eq!(state.mode, EffectiveMode::Light);
        assert_eq!(state.reason, Some(ModeReason::Cpu));
    }

    #[test]
    fn two_lagging_samples_step_down() {
        let mut c = controller();
        assert_eq!(c.sample(0.0, 10.0, 3.0), None);
        let state = c.sample(1.0, 10.0, 4.0).unwrap();
        assert_eq!(state.reason, Some(ModeReason::Lag));
        let mut c = controller();
        c.sample(0.0, 10.0, 3.0);
        assert_eq!(c.sample(1.0, 10.0, 2.9), None);
        assert_eq!(c.sample(2.0, 10.0, 3.0), None);
    }

    #[test]
    fn stepping_up_needs_ten_quiet_samples_low_lag_and_ten_seconds() {
        let mut c = controller();
        down(&mut c, 0.0);
        // quiet from t = 3; the tenth quiet sample is at t = 12 (9 s after the switch at t = 2)
        for second in 3..12 {
            assert_eq!(c.sample(f64::from(second), 20.0, 0.0), None, "t = {second}");
        }
        let state = c.sample(12.0, 20.0, 0.0).unwrap();
        assert_eq!(state.mode, EffectiveMode::Continuous);
        assert_eq!(
            state.reason,
            Some(ModeReason::Auto),
            "back to how auto resolved"
        );
    }

    #[test]
    fn high_lag_or_busy_cpu_blocks_stepping_up() {
        let mut c = controller();
        down(&mut c, 0.0);
        for second in 3..30 {
            let lag = if second == 20 { 2.0 } else { 0.0 };
            let _ = c.sample(f64::from(second), 20.0, lag);
        }
        // the lag sample at t = 20 means the quiet count (needs lag < 1.5 at the moment) held up
        // nothing permanently: by now the controller has returned.
        assert_eq!(c.state().mode, EffectiveMode::Continuous);
        let mut c = controller();
        down(&mut c, 0.0);
        for second in 3..40 {
            // 65% is not below the step-up threshold
            assert_eq!(c.sample(f64::from(second), 65.0, 0.0), None);
        }
        assert_eq!(c.state().mode, EffectiveMode::Light);
    }

    #[test]
    fn three_step_downs_in_five_minutes_lock_light() {
        let mut c = controller();
        down(&mut c, 0.0);
        for second in 3..14 {
            c.sample(f64::from(second), 10.0, 0.0);
        }
        assert_eq!(c.state().mode, EffectiveMode::Continuous);
        down(&mut c, 20.0);
        for second in 23..34 {
            c.sample(f64::from(second), 10.0, 0.0);
        }
        assert_eq!(c.state().mode, EffectiveMode::Continuous);
        down(&mut c, 40.0);
        assert!(c.is_locked());
        for second in 43..400 {
            assert_eq!(c.sample(f64::from(second), 1.0, 0.0), None);
        }
        assert_eq!(c.state().mode, EffectiveMode::Light);
        assert_eq!(c.state().reason, Some(ModeReason::Cpu));
    }

    #[test]
    fn step_downs_spread_over_more_than_five_minutes_do_not_lock() {
        let mut c = controller();
        for round in 0..4 {
            let start = f64::from(round) * 400.0;
            down(&mut c, start);
            for second in 3..14 {
                c.sample(start + f64::from(second), 10.0, 0.0);
            }
            assert_eq!(c.state().mode, EffectiveMode::Continuous, "round {round}");
        }
        assert!(!c.is_locked());
    }

    #[test]
    fn the_controller_can_be_switched_off() {
        let config = LatencyConfig {
            step_down: false,
            ..LatencyConfig::default()
        };
        let mut c = StepDown::new(&config, EffectiveMode::Continuous, None);
        for second in 0..20 {
            assert_eq!(c.sample(f64::from(second), 100.0, 9.0), None);
        }
        assert_eq!(c.state().mode, EffectiveMode::Continuous);
    }

    #[test]
    fn light_and_off_never_change() {
        for mode in [EffectiveMode::Light, EffectiveMode::Off] {
            let mut c = StepDown::new(&LatencyConfig::default(), mode, Some(ModeReason::User));
            for second in 0..20 {
                assert_eq!(c.sample(f64::from(second), 100.0, 9.0), None);
            }
            assert_eq!(c.state().mode, mode);
        }
    }

    #[test]
    fn three_draft_failures_switch_to_light_until_the_server_is_ready() {
        let mut c = controller();
        assert_eq!(c.draft_failed(), None);
        assert_eq!(c.draft_failed(), None);
        c.draft_succeeded();
        assert_eq!(c.draft_failed(), None);
        assert_eq!(c.draft_failed(), None);
        let state = c.draft_failed().unwrap();
        assert_eq!(state.mode, EffectiveMode::Light);
        assert_eq!(state.reason, Some(ModeReason::DraftUnavailable));
        // CPU samples cannot bring it back while the draft server is unavailable
        for second in 0..30 {
            assert_eq!(c.sample(f64::from(second), 1.0, 0.0), None);
        }
        let back = c.draft_server(true).unwrap();
        assert_eq!(back.mode, EffectiveMode::Continuous);
        assert_eq!(back.reason, Some(ModeReason::Auto));
    }

    #[test]
    fn a_failed_draft_server_switches_immediately() {
        let mut c = controller();
        let state = c.draft_server(false).unwrap();
        assert_eq!(state.reason, Some(ModeReason::DraftUnavailable));
    }

    #[test]
    fn choosing_a_mode_resets_the_controller() {
        let mut c = controller();
        down(&mut c, 0.0);
        let state = c.choose(EffectiveMode::Continuous, 5.0).unwrap();
        assert_eq!(state.mode, EffectiveMode::Continuous);
        assert_eq!(state.reason, Some(ModeReason::User));
        let state = c.choose(EffectiveMode::Light, 6.0).unwrap();
        assert_eq!(state.mode, EffectiveMode::Light);
    }
}

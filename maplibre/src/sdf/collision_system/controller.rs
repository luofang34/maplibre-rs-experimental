//! Throttles screen-space symbol collision passes to discrete placement epochs.
//!
//! Matches MapLibre Native / GL JS placement cadence:
//! - Continuous camera motion places at most every `250 ms` (`~4 Hz`).
//! - Newly uploaded symbol buckets, style/terrain surface updates, integer zoom crossings, and
//!   repeat-spacing settle passes debounce at `30 ms`.
//! - Stationary settled views run at `0 Hz` (`0` placement passes, `0` GPU buffer writes).
//! - Deterministic `PlacementConfig::immediate()` (`fade_duration == Duration::ZERO`) places on
//!   every view or content change without throttling.

use std::time::Duration;

/// Cadence during continuous pan, tilt, rotate, or zoom gestures (`~4 Hz`).
pub(super) const CONTINUOUS_PLACEMENT_PERIOD: Duration = Duration::from_millis(250);

/// Fast debounce when newly uploaded symbol tiles arrive or integer zoom levels cross.
pub(super) const FAST_PLACEMENT_PERIOD: Duration = Duration::from_millis(30);

/// World resource configuring symbol placement cadence and GPU fade duration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlacementConfig {
    /// Duration of GPU-interpolated symbol collision opacity fades (`160 ms` by default).
    /// Setting [`Duration::ZERO`] disables throttling and fades for deterministic snapshot tests.
    pub fade_duration: Duration,
}

impl Default for PlacementConfig {
    fn default() -> Self {
        Self {
            fade_duration: Duration::from_millis(160),
        }
    }
}

impl PlacementConfig {
    /// Immediate deterministic placement mode (`fade_duration == Duration::ZERO`).
    pub const fn immediate() -> Self {
        Self {
            fade_duration: Duration::ZERO,
        }
    }
}

/// Frame-level signals inspected by [`PlacementController::should_place`].
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct PlacementTriggers {
    /// Standard camera view-projection or viewport size changed since the last placement epoch.
    pub moved: bool,
    /// Standard camera view-projection or viewport size changed since the immediately preceding frame.
    pub moving_this_frame: bool,
    /// Host drives the camera externally (e.g. head-tracked XR).
    pub external_view: bool,
    /// External XR eye matrix changed since the last frame.
    pub external_moved: bool,
    /// Resident symbol buckets, feature counts, or style paint revisions changed.
    pub new_content: bool,
    /// Terrain DEM coverage or camera tracking mode changed.
    pub environment_changed: bool,
    /// Host `LabelObstacles` revision changed.
    pub obstructed: bool,
}

/// Decides whether the current frame executes a full CPU symbol collision epoch.
#[derive(Debug, Default)]
pub(super) struct PlacementController {
    initialized: bool,
    prev_raw_now: Option<Duration>,
    clock: Duration,
    last_commit_clock: Duration,
    last_commit_zoom: f64,
    stale: bool,
    fast_followup: bool,
    deferred_budget_frames: u8,
}

impl PlacementController {
    /// Whether a throttled placement epoch is still waiting for its debounce window to elapse.
    pub(super) fn is_pending(&self) -> bool {
        self.stale
    }

    /// Whether a moving frame whose earlier stages already consumed most of a 16.6 ms budget may
    /// defer a non-initial symbol placement pass by one frame without starving placement.
    pub(super) fn defer_for_frame_budget(&mut self, triggers: PlacementTriggers) -> bool {
        if !self.initialized || triggers.obstructed || self.deferred_budget_frames >= 2 {
            return false;
        }
        self.deferred_budget_frames = self.deferred_budget_frames.saturating_add(1);
        self.stale = true;
        self.fast_followup |= triggers.new_content || triggers.environment_changed;
        true
    }

    /// Evaluates whether `CollisionSystem` should run a placement epoch on this frame.
    pub(super) fn should_place(
        &mut self,
        now: Duration,
        zoom: f64,
        config: PlacementConfig,
        triggers: PlacementTriggers,
    ) -> bool {
        let first_tick = self.prev_raw_now.is_none();
        let dt = match self.prev_raw_now {
            Some(prev) if now > prev => now - prev,
            _ => Duration::ZERO,
        };
        self.prev_raw_now = Some(now);
        self.clock = self.clock.saturating_add(dt);

        if !self.initialized || triggers.obstructed {
            return true;
        }

        let any_change = triggers.moved
            || triggers.external_moved
            || triggers.new_content
            || triggers.environment_changed;
        if config.fade_duration.is_zero() && (any_change || self.stale) {
            return true;
        }
        // Single-step headless/XR callers that keep `FrameInput.timestamp` constant expect
        // synchronous placement on explicit camera or content mutations.
        if !first_tick && dt.is_zero() && any_change {
            return true;
        }

        let integer_zoom_crossed = (zoom.floor() as i32) != (self.last_commit_zoom.floor() as i32);
        let settle_fast =
            triggers.new_content || triggers.environment_changed || self.fast_followup;
        // During active continuous camera motion (`moving_this_frame`), streaming tile arrivals and
        // repeat-spacing followups must NOT collapse the 250 ms (`4 Hz`) cadence down to 30 ms
        // (`33 Hz`). Record `fast_followup` so once camera motion stops, a crisp 30 ms settle pass
        // still fires immediately.
        let fast = integer_zoom_crossed || (!triggers.moving_this_frame && settle_fast);
        let dirty = self.stale || settle_fast || fast || triggers.moved || triggers.external_view;
        if !dirty {
            return false;
        }

        let period = if fast {
            FAST_PLACEMENT_PERIOD
        } else {
            CONTINUOUS_PLACEMENT_PERIOD
        };
        if self.clock.saturating_sub(self.last_commit_clock) >= period {
            true
        } else {
            self.stale = true;
            self.fast_followup |= settle_fast || integer_zoom_crossed;
            false
        }
    }

    /// Records the outcome of a placement epoch.
    pub(super) fn record_commit(&mut self, zoom: f64, has_symbols: bool, followup_needed: bool) {
        self.deferred_budget_frames = 0;
        if !has_symbols {
            // Keep uninitialized across empty warmup frames so the first frame with actual
            // symbol buckets places immediately without waiting for a 250 ms timer.
            self.initialized = false;
            self.stale = false;
            self.fast_followup = false;
            return;
        }
        self.initialized = true;
        self.last_commit_clock = self.clock;
        self.last_commit_zoom = zoom;
        self.stale = followup_needed;
        self.fast_followup = followup_needed;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continuous_camera_motion_throttles_to_250ms_and_settles_to_zero_hz() {
        let mut ctrl = PlacementController::default();
        let cfg = PlacementConfig::default();

        // Initial frame with symbols places immediately.
        assert!(ctrl.should_place(
            Duration::from_millis(0),
            14.1,
            cfg,
            PlacementTriggers {
                moved: true,
                new_content: true,
                ..Default::default()
            }
        ));
        ctrl.record_commit(14.1, true, false);

        // 60 Hz pan frames within 250 ms are skipped.
        for ms in [16_u64, 32, 64, 128, 240] {
            assert!(
                !ctrl.should_place(
                    Duration::from_millis(ms),
                    14.2,
                    cfg,
                    PlacementTriggers {
                        moved: true,
                        ..Default::default()
                    }
                ),
                "expected skip at {ms} ms"
            );
            assert!(ctrl.is_pending());
        }

        // At 256 ms (>= 250 ms), the next placement epoch fires.
        assert!(ctrl.should_place(
            Duration::from_millis(256),
            14.2,
            cfg,
            PlacementTriggers {
                moved: true,
                ..Default::default()
            }
        ));
        ctrl.record_commit(14.2, true, false);
        assert!(!ctrl.is_pending());

        // Once camera motion stops, settled frames stay at 0 Hz indefinitely.
        for ms in [272_u64, 400, 600, 1_000, 5_000] {
            assert!(!ctrl.should_place(
                Duration::from_millis(ms),
                14.2,
                cfg,
                PlacementTriggers::default()
            ));
            assert!(!ctrl.is_pending());
        }
    }

    #[test]
    fn new_content_and_integer_zoom_crossings_use_30ms_fast_debounce() {
        let mut ctrl = PlacementController::default();
        let cfg = PlacementConfig::default();

        assert!(ctrl.should_place(
            Duration::ZERO,
            13.9,
            cfg,
            PlacementTriggers {
                new_content: true,
                ..Default::default()
            }
        ));
        ctrl.record_commit(13.9, true, false);

        // New tile arrives 16 ms later: debounced until 30 ms.
        assert!(!ctrl.should_place(
            Duration::from_millis(16),
            13.9,
            cfg,
            PlacementTriggers {
                new_content: true,
                ..Default::default()
            }
        ));
        assert!(ctrl.should_place(
            Duration::from_millis(32),
            13.9,
            cfg,
            PlacementTriggers::default()
        ));
        ctrl.record_commit(13.9, true, false);

        // Crossing integer zoom 13.9 -> 14.0 also triggers after 30 ms rather than 250 ms.
        assert!(!ctrl.should_place(
            Duration::from_millis(48),
            14.05,
            cfg,
            PlacementTriggers {
                moved: true,
                ..Default::default()
            }
        ));
        assert!(ctrl.should_place(
            Duration::from_millis(64),
            14.05,
            cfg,
            PlacementTriggers {
                moved: true,
                ..Default::default()
            }
        ));
    }

    #[test]
    fn empty_warmup_frames_do_not_delay_first_symbol_tile() {
        let mut ctrl = PlacementController::default();
        let cfg = PlacementConfig::default();

        assert!(ctrl.should_place(Duration::ZERO, 14.0, cfg, PlacementTriggers::default()));
        ctrl.record_commit(14.0, false, false);

        // First real symbol tile at 16 ms places immediately without waiting 30/250 ms.
        assert!(ctrl.should_place(
            Duration::from_millis(16),
            14.0,
            cfg,
            PlacementTriggers {
                new_content: true,
                ..Default::default()
            }
        ));
    }

    #[test]
    fn immediate_config_places_every_changed_frame_without_debounce() {
        let mut ctrl = PlacementController::default();
        let cfg = PlacementConfig::immediate();

        assert!(ctrl.should_place(Duration::ZERO, 14.0, cfg, PlacementTriggers::default()));
        ctrl.record_commit(14.0, true, false);

        assert!(ctrl.should_place(
            Duration::from_millis(5),
            14.0,
            cfg,
            PlacementTriggers {
                moved: true,
                ..Default::default()
            }
        ));
    }

    #[test]
    fn streaming_tiles_during_continuous_motion_keep_250ms_cadence_then_settle_fast() {
        let mut ctrl = PlacementController::default();
        let cfg = PlacementConfig::default();

        assert!(ctrl.should_place(
            Duration::ZERO,
            14.2,
            cfg,
            PlacementTriggers {
                moved: true,
                moving_this_frame: true,
                new_content: true,
                ..Default::default()
            }
        ));
        // Even with `followup_needed: true` and continuous `new_content: true`, active motion must
        // stay at 250 ms (`4 Hz`) rather than collapsing to 30 ms (`33 Hz`).
        ctrl.record_commit(14.2, true, true);

        for ms in [16_u64, 32, 48, 80, 128, 200, 240] {
            assert!(
                !ctrl.should_place(
                    Duration::from_millis(ms),
                    14.3,
                    cfg,
                    PlacementTriggers {
                        moved: true,
                        moving_this_frame: true,
                        new_content: true,
                        ..Default::default()
                    }
                ),
                "streaming tile arrival during continuous flight must not trigger early placement at {ms} ms"
            );
        }

        // At 256 ms (>= 250 ms), the next 4 Hz motion epoch commits.
        assert!(ctrl.should_place(
            Duration::from_millis(256),
            14.3,
            cfg,
            PlacementTriggers {
                moved: true,
                moving_this_frame: true,
                new_content: true,
                ..Default::default()
            }
        ));
        ctrl.record_commit(14.3, true, false);

        // One more tile uploads while moving at 272 ms, then camera stops at 290 ms (> 30 ms after 256 ms).
        assert!(!ctrl.should_place(
            Duration::from_millis(272),
            14.35,
            cfg,
            PlacementTriggers {
                moved: true,
                moving_this_frame: true,
                new_content: true,
                ..Default::default()
            }
        ));
        assert!(ctrl.should_place(
            Duration::from_millis(290),
            14.35,
            cfg,
            PlacementTriggers {
                moved: true,
                moving_this_frame: false,
                ..Default::default()
            }
        ));
    }
}

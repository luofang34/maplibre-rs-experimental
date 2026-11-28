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
    ever_placed: bool,
    prev_raw_now: Option<Duration>,
    last_dt_non_zero: bool,
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

    /// Whether at least one non-empty placement epoch has already committed.
    pub(super) fn is_initialized(&self) -> bool {
        self.initialized
    }

    /// Whether any non-empty placement epoch has ever committed (survives transient empty frames).
    pub(super) fn has_ever_placed(&self) -> bool {
        self.ever_placed
    }

    /// Whether the most recent frame advanced `FrameInput.timestamp` (`dt > 0`).
    pub(super) fn last_dt_non_zero(&self) -> bool {
        self.last_dt_non_zero
    }

    /// Advances the internal monotonic clock while an in-progress time-sliced epoch continues.
    pub(super) fn advance_clock(&mut self, now: Duration) {
        let dt = match self.prev_raw_now {
            Some(prev) if now > prev => now - prev,
            _ => Duration::ZERO,
        };
        self.prev_raw_now = Some(now);
        self.last_dt_non_zero = !dt.is_zero();
        self.clock = self.clock.saturating_add(dt);
    }

    /// Remembers content, environment, or zoom changes that arrived while a multi-frame slice was
    /// already in progress so the next debounce check runs once the current epoch commits.
    pub(super) fn note_background_dirty(&mut self, zoom: f64, triggers: PlacementTriggers) {
        let integer_zoom_crossed = (zoom.floor() as i32) != (self.last_commit_zoom.floor() as i32);
        let fast_change =
            triggers.new_content || triggers.environment_changed || integer_zoom_crossed;
        if fast_change || triggers.moved || triggers.external_moved {
            self.stale = true;
            self.fast_followup |= fast_change;
        }
    }

    /// Whether a moving frame whose earlier stages already consumed most of a 16.6 ms budget may
    /// defer a non-initial symbol placement pass by one frame without starving placement.
    /// Single-step headless callers (`dt == 0`) never defer.
    pub(super) fn defer_for_frame_budget(&mut self, triggers: PlacementTriggers) -> bool {
        if !self.initialized
            || !self.last_dt_non_zero
            || triggers.obstructed
            || self.deferred_budget_frames >= 2
        {
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
        self.last_dt_non_zero = !dt.is_zero();
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
        let settle_fast = triggers.new_content
            || triggers.environment_changed
            || integer_zoom_crossed
            || self.fast_followup;
        // During active continuous camera motion (`moving_this_frame`), streaming tile arrivals,
        // integer zoom crossings, and repeat-spacing followups must NOT collapse the 250 ms
        // (`4 Hz`) cadence down to 30 ms (`33 Hz`). Record `fast_followup` so once camera motion
        // stops, a crisp 30 ms settle pass still fires immediately.
        let fast = !triggers.moving_this_frame && settle_fast;
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
            self.fast_followup |= settle_fast;
            false
        }
    }

    /// Records that an in-flight epoch was aborted by rapid camera motion so restarts keep the
    /// `250 ms` motion cadence and settle immediately once motion pauses.
    pub(super) fn note_aborted_motion(&mut self, zoom: f64) {
        self.stale = true;
        self.fast_followup = true;
        self.last_commit_clock = self.clock;
        self.last_commit_zoom = zoom;
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
        self.ever_placed = true;
        self.last_commit_clock = self.clock;
        self.last_commit_zoom = zoom;
        self.stale = followup_needed;
        self.fast_followup = followup_needed;
    }
}

#[cfg(test)]
mod tests;

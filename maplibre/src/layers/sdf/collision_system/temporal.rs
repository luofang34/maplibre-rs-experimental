//! Placement history follows a symbol across tile replacement, with GPU-interpolated fades.
use std::time::Duration;

pub(in crate::layers::sdf::collision_system) mod repeat_spacing;
mod state;

use rustc_hash::FxHashMap;
use state::{feature_hash, tile_scale, State};

use crate::{
    coords::WorldTileCoords,
    sdf::{Feature, SymbolLayerData},
};

/// Default duration in milliseconds of a symbol collision opacity transition (`160 ms`).
pub(crate) const DEFAULT_FADE_DURATION_MS: f32 = 160.0;

/// Linear opacity transition evaluated in the SDF vertex shader against `(u_fade_time_ms, u_fade_duration_ms)`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(in crate::layers::sdf::collision_system) struct FadeRamp {
    pub start: f32,
    pub target: f32,
    pub start_time_ms: f32,
}

impl FadeRamp {
    pub(in crate::layers::sdf::collision_system) fn is_hidden(self) -> bool {
        self.start <= 0.0 && self.target <= 0.0
    }

    pub(in crate::layers::sdf::collision_system) fn scale(self, factor: f32) -> Self {
        if factor <= 0.0 || self.is_hidden() {
            Self::default()
        } else {
            Self {
                start: self.start * factor,
                target: self.target * factor,
                start_time_ms: self.start_time_ms,
            }
        }
    }

    pub(in crate::layers::sdf::collision_system) fn at(
        self,
        now_ms: f32,
        fade_duration_ms: f32,
    ) -> f32 {
        if fade_duration_ms <= 0.0 || (self.start - self.target).abs() <= f32::EPSILON {
            return self.target.clamp(0.0, 1.0);
        }
        let t = ((now_ms - self.start_time_ms) / fade_duration_ms).clamp(0.0, 1.0);
        (self.start + (self.target - self.start) * t).clamp(0.0, 1.0)
    }
}

pub(in crate::layers::sdf::collision_system) struct PlacementHistory {
    states: FxHashMap<String, FxHashMap<u64, Vec<State>>>,
    now: Duration,
    prev_now: Option<Duration>,
    active_frame_touched_at: Duration,
    clock_ms: f32,
    fade_until_ms: f32,
    fade_duration_ms: f32,
    frame: u64,
    /// Where the repeats of labels along lines were placed on screen.
    repeats: repeat_spacing::RepeatSpacing,
}

impl Default for PlacementHistory {
    fn default() -> Self {
        Self {
            states: FxHashMap::default(),
            now: Duration::ZERO,
            prev_now: None,
            active_frame_touched_at: Duration::ZERO,
            clock_ms: 0.0,
            fade_until_ms: 0.0,
            fade_duration_ms: DEFAULT_FADE_DURATION_MS,
            frame: 0,
            repeats: repeat_spacing::RepeatSpacing::default(),
        }
    }
}

fn clock_delta_ms(prev: Duration, now: Duration, was_fading: bool) -> f32 {
    if now <= prev {
        return 0.0;
    }
    let delta_ms = ((now - prev).as_secs_f64() * 1000.0) as f32;
    if was_fading {
        delta_ms
    } else {
        delta_ms.min(500.0)
    }
}

fn tile_center_and_half_span(tile: WorldTileCoords) -> (f64, f64, f64) {
    let scale = tile_scale(u8::from(tile.z));
    (
        (f64::from(tile.x) + 0.5) / scale,
        (f64::from(tile.y) + 0.5) / scale,
        0.76 / scale,
    )
}

impl PlacementHistory {
    fn find_state(&self, layer: &SymbolLayerData, feature: &Feature) -> Option<&State> {
        self.states
            .get(layer.style_layer_id.as_str())?
            .get(&feature_hash(feature))?
            .iter()
            .find(|state| state.matches(layer.coords, feature))
    }

    pub(in crate::layers::sdf::collision_system) fn yielded_to_previous_repeat(&self) -> bool {
        self.repeats.yielded_to_previous()
    }

    pub(in crate::layers::sdf::collision_system) fn repeat_too_close(
        &self,
        repeat: &repeat_spacing::Repeat,
        was_visible: bool,
    ) -> bool {
        self.repeats.too_close(repeat, was_visible)
    }

    pub(in crate::layers::sdf::collision_system) fn remember_repeat(
        &mut self,
        repeat: repeat_spacing::Repeat,
    ) {
        self.repeats.remember(repeat);
    }

    pub(in crate::layers::sdf::collision_system) fn set_fade_duration(
        &mut self,
        duration: Duration,
    ) {
        self.fade_duration_ms = (duration.as_secs_f64() * 1000.0) as f32;
    }

    pub(in crate::layers::sdf::collision_system) fn fade_duration_ms(&self) -> f32 {
        self.fade_duration_ms
    }

    /// Advances the relative monotonic fade clock in milliseconds, capping idle gaps when no fade
    /// is active so long browser-tab idle periods never exhaust compact GPU timestamp bits.
    pub(in crate::layers::sdf::collision_system) fn advance_clock(&mut self, now: Duration) -> f32 {
        let was_fading = self.is_fading();
        if let Some(prev) = self.prev_now {
            self.clock_ms += clock_delta_ms(prev, now, was_fading);
        }
        self.prev_now = Some(now);
        self.now = now;
        self.clock_ms
    }

    /// Records the latest timestamp for states claimed by the most recent placement epoch so
    /// throttled/skipped frames stay O(1) and expire lazily at the next epoch commit.
    pub(in crate::layers::sdf::collision_system) fn touch_active_states(&mut self, now: Duration) {
        if now > self.active_frame_touched_at {
            self.active_frame_touched_at = now;
        }
    }

    /// Rebases the monotonic millisecond fade clock back to `0.0` once every fade has settled and
    /// the clock approaches half of the 19-bit GPU `start_ticks` limit (`2_097_148 ms`).
    pub(in crate::layers::sdf::collision_system) fn rebase_clock_if_idle(&mut self) -> bool {
        if self.is_fading() || self.clock_ms < 1_000_000.0 {
            return false;
        }
        self.clock_ms = 0.0;
        self.fade_until_ms = 0.0;
        for layer_states in self.states.values_mut() {
            for states in layer_states.values_mut() {
                for state in states.iter_mut() {
                    state.rebase_settled();
                }
            }
        }
        true
    }

    pub(in crate::layers::sdf::collision_system) fn is_fading(&self) -> bool {
        self.fade_duration_ms > 0.0 && self.clock_ms < self.fade_until_ms
    }

    /// Shader clock value that advances during active fades and stays bit-identical once settled
    /// so `SymbolDepth::write_fade_time` issues zero uniform writes on settled frames while still
    /// syncing newly created/swapped `SymbolDepth` attachments automatically.
    pub(in crate::layers::sdf::collision_system) fn effective_shader_time_ms(&self) -> f32 {
        if self.is_fading() {
            self.clock_ms
        } else {
            // +4.0 ms guarantees the shader clock clears any 4 ms `start_ticks` step + rounding.
            self.fade_until_ms + 4.0
        }
    }

    #[cfg(test)]
    pub(in crate::layers::sdf::collision_system) fn begin(&mut self, now: Duration) {
        self.begin_with_duration(now, Duration::from_millis(160));
    }

    pub(in crate::layers::sdf::collision_system) fn begin_with_duration(
        &mut self,
        now: Duration,
        fade_duration: Duration,
    ) {
        self.set_fade_duration(fade_duration);
        self.advance_clock(now);
        let prev_frame = self.frame;
        let touched = self.active_frame_touched_at;
        self.active_frame_touched_at = now;
        self.frame = self.frame.wrapping_add(1);
        self.repeats.begin();
        let retention = Duration::from_secs(1).max(fade_duration);
        self.states.retain(|_, layer_states| {
            layer_states.retain(|_, states| {
                states.retain_mut(|state| {
                    state.touch_and_is_retained(prev_frame, touched, now, retention)
                });
                !states.is_empty()
            });
            !layer_states.is_empty()
        });
    }

    pub(in crate::layers::sdf::collision_system) fn was_visible(
        &self,
        layer: &SymbolLayerData,
        feature: &Feature,
    ) -> bool {
        self.find_state(layer, feature)
            .is_some_and(State::is_target_visible)
    }

    /// Whether any visible or actively fading symbol state for `style_layer_id` could match a
    /// feature inside `tile` (including tile margin padding), allowing horizon/off-screen tiles
    /// without active fades to skip per-feature projection completely.
    pub(in crate::layers::sdf::collision_system) fn tile_has_active_state(
        &self,
        style_layer_id: &str,
        tile: WorldTileCoords,
    ) -> bool {
        let Some(layer_states) = self.states.get(style_layer_id) else {
            return false;
        };
        let (center_x, center_y, half_span) = tile_center_and_half_span(tile);
        layer_states.values().flatten().any(|state| {
            state.is_active() && state.overlaps_tile(tile, center_x, center_y, half_span)
        })
    }

    /// Whether another tile's copy of the label already took it this frame, so this copy is
    /// neither drawn nor found by a query. Asked before [`Self::fade_ramps`] takes it.
    pub(in crate::layers::sdf::collision_system) fn shown_elsewhere(
        &self,
        layer: &SymbolLayerData,
        feature: &Feature,
    ) -> bool {
        let Some(layer_states) = self.states.get(layer.style_layer_id.as_str()) else {
            return false;
        };
        let frame = self.frame;
        layer_states
            .get(&feature_hash(feature))
            .is_some_and(|states| {
                states
                    .iter()
                    .find(|state| {
                        state.matches(layer.coords, feature)
                            && !state.is_claimed_by_tile(frame, layer.coords)
                    })
                    .is_some_and(|state| state.is_claimed_in_frame(frame))
            })
    }

    pub(in crate::layers::sdf::collision_system) fn fade_ramps(
        &mut self,
        layer: &SymbolLayerData,
        feature: &Feature,
        target: [bool; 2],
    ) -> [FadeRamp; 2] {
        let frame = self.frame;
        let now_ms = self.clock_ms;
        let fade_duration_ms = self.fade_duration_ms;
        let immediate = fade_duration_ms <= 0.0;
        let any_target = target[0] || target[1];
        let hash = feature_hash(feature);
        if !any_target {
            let Some(layer_states) = self.states.get(layer.style_layer_id.as_str()) else {
                return [FadeRamp::default(); 2];
            };
            if !layer_states.contains_key(&hash) {
                return [FadeRamp::default(); 2];
            }
        }
        let states = self
            .states
            .entry(layer.style_layer_id.clone())
            .or_default()
            .entry(hash)
            .or_default();
        // A state a same-tile feature already took this frame belongs to that feature, so a
        // second feature of the tile with the same key is tracked separately.
        let existing = states.iter().position(|state| {
            state.matches(layer.coords, feature) && !state.is_claimed_by_tile(frame, layer.coords)
        });
        let Some(index) = existing.or_else(|| {
            any_target.then(|| {
                states.push(State::new_for_target(
                    layer,
                    feature,
                    target,
                    self.now,
                    now_ms,
                    immediate,
                    frame.wrapping_sub(1),
                ));
                states.len() - 1
            })
        }) else {
            return [FadeRamp::default(); 2];
        };
        let Some(state) = states.get_mut(index) else {
            return [FadeRamp::default(); 2];
        };
        // A label repeated across tiles is drawn once, so a parent and child that are both
        // visible do not double up.
        if state.is_claimed_in_frame(frame) {
            return [FadeRamp::default(); 2];
        }
        let fade_until = state.update_ramps(target, now_ms, fade_duration_ms);
        self.fade_until_ms = self.fade_until_ms.max(fade_until);
        state.claim_for_frame(target, self.now, frame, layer.coords, feature);
        state.ramps()
    }

    #[cfg(test)]
    pub(in crate::layers::sdf::collision_system) fn opacity(
        &mut self,
        layer: &SymbolLayerData,
        feature: &Feature,
        target: [bool; 2],
    ) -> [f32; 2] {
        let now_ms = self.clock_ms;
        let fade_duration_ms = self.fade_duration_ms;
        self.fade_ramps(layer, feature, target)
            .map(|ramp| ramp.at(now_ms, fade_duration_ms))
    }
}

impl PlacementHistory {
    /// The anchor the label's text was placed with while it was last shown or fading out, which
    /// GL JS tries first so a label that still fits does not jump between anchors.
    pub(in crate::layers::sdf::collision_system) fn previous_anchor(
        &self,
        layer: &SymbolLayerData,
        feature: &Feature,
    ) -> Option<usize> {
        self.find_state(layer, feature)?.anchor()
    }

    /// Records the anchor this frame placed the label's text with, on the state
    /// [`Self::fade_ramps`] took for it. A label not placed keeps its anchor while it fades out,
    /// as GL JS keeps the offsets of symbols that are not yet hidden.
    pub(in crate::layers::sdf::collision_system) fn remember_anchor(
        &mut self,
        layer: &SymbolLayerData,
        feature: &Feature,
        anchor: Option<usize>,
    ) {
        let frame = self.frame;
        let now_ms = self.clock_ms;
        let fade_duration_ms = self.fade_duration_ms;
        if let Some(state) = self
            .states
            .get_mut(layer.style_layer_id.as_str())
            .and_then(|layer_states| layer_states.get_mut(&feature_hash(feature)))
            .and_then(|states| {
                states.iter_mut().find(|state| {
                    state.is_claimed_by_tile(frame, layer.coords)
                        && state.matches(layer.coords, feature)
                })
            })
        {
            state.update_anchor(anchor, now_ms, fade_duration_ms);
        }
    }
}

#[cfg(test)]
mod tests;

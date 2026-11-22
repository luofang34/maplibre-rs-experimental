//! Placement history follows a symbol across tile replacement, with GPU-interpolated fades.
use std::{
    hash::{Hash, Hasher},
    time::Duration,
};

use rustc_hash::{FxHashMap, FxHasher};

use crate::{
    coords::WorldTileCoords,
    sdf::{Feature, SymbolLayerData},
};

/// Default duration in milliseconds of a symbol collision opacity transition (`160 ms`).
pub(crate) const DEFAULT_FADE_DURATION_MS: f32 = 160.0;

/// Linear opacity transition evaluated in the SDF vertex shader against `(u_fade_time_ms, u_fade_duration_ms)`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct FadeRamp {
    pub start: f32,
    pub target: f32,
    pub start_time_ms: f32,
}

impl FadeRamp {
    pub(super) fn is_hidden(self) -> bool {
        self.start <= 0.0 && self.target <= 0.0
    }

    pub(super) fn scale(self, factor: f32) -> Self {
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

    pub(super) fn at(self, now_ms: f32, fade_duration_ms: f32) -> f32 {
        if fade_duration_ms <= 0.0 || (self.start - self.target).abs() <= f32::EPSILON {
            return self.target.clamp(0.0, 1.0);
        }
        let t = ((now_ms - self.start_time_ms) / fade_duration_ms).clamp(0.0, 1.0);
        (self.start + (self.target - self.start) * t).clamp(0.0, 1.0)
    }
}

#[derive(Clone)]
struct State {
    text: String,
    id: Option<u64>,
    position: [f64; 2],
    zoom: u8,
    ramp: [FadeRamp; 2],
    target: [bool; 2],
    last_seen: Duration,
    claimed: u64,
    claimed_by: WorldTileCoords,
    /// The variable anchor the text was last placed with.
    anchor: Option<usize>,
}

pub(super) struct PlacementHistory {
    states: FxHashMap<String, FxHashMap<u64, Vec<State>>>,
    now: Duration,
    prev_now: Option<Duration>,
    clock_ms: f32,
    fade_until_ms: f32,
    fade_duration_ms: f32,
    frame: u64,
    /// Where the repeats of labels along lines were placed on screen.
    pub(super) repeats: super::repeat_spacing::RepeatSpacing,
}

impl Default for PlacementHistory {
    fn default() -> Self {
        Self {
            states: FxHashMap::default(),
            now: Duration::ZERO,
            prev_now: None,
            clock_ms: 0.0,
            fade_until_ms: 0.0,
            fade_duration_ms: DEFAULT_FADE_DURATION_MS,
            frame: 0,
            repeats: super::repeat_spacing::RepeatSpacing::default(),
        }
    }
}

impl PlacementHistory {
    pub(super) fn set_fade_duration(&mut self, duration: Duration) {
        self.fade_duration_ms = (duration.as_secs_f64() * 1000.0) as f32;
    }

    pub(super) fn fade_duration_ms(&self) -> f32 {
        self.fade_duration_ms
    }

    /// Advances the relative monotonic fade clock in milliseconds, capping idle gaps when no fade
    /// is active so long browser-tab idle periods never exhaust compact GPU timestamp bits.
    pub(super) fn advance_clock(&mut self, now: Duration) -> f32 {
        let was_fading = self.is_fading();
        if let Some(prev) = self.prev_now {
            if now > prev {
                let delta_ms = ((now - prev).as_secs_f64() * 1000.0) as f32;
                self.clock_ms += if was_fading {
                    delta_ms
                } else {
                    delta_ms.min(500.0)
                };
            }
        }
        self.prev_now = Some(now);
        self.now = now;
        self.clock_ms
    }

    /// Keeps states claimed by the most recent placement epoch alive across idle frames that
    /// intentionally skip running a new placement pass.
    pub(super) fn touch_active_states(&mut self, now: Duration) {
        let frame = self.frame;
        for layer_states in self.states.values_mut() {
            for states in layer_states.values_mut() {
                for state in states.iter_mut() {
                    if state.claimed == frame && now > state.last_seen {
                        state.last_seen = now;
                    }
                }
            }
        }
    }

    /// Rebases the monotonic millisecond fade clock back to `0.0` once every fade has settled and
    /// the clock approaches half of the 19-bit GPU `start_ticks` limit (`2_097_148 ms`).
    pub(super) fn rebase_clock_if_idle(&mut self) -> bool {
        if self.is_fading() || self.clock_ms < 1_000_000.0 {
            return false;
        }
        self.clock_ms = 0.0;
        self.fade_until_ms = 0.0;
        for layer_states in self.states.values_mut() {
            for states in layer_states.values_mut() {
                for state in states.iter_mut() {
                    for ramp in &mut state.ramp {
                        ramp.start = ramp.target;
                        ramp.start_time_ms = 0.0;
                    }
                }
            }
        }
        true
    }

    pub(super) fn is_fading(&self) -> bool {
        self.fade_duration_ms > 0.0 && self.clock_ms < self.fade_until_ms
    }

    /// Shader clock value that advances during active fades and stays bit-identical once settled
    /// so `SymbolDepth::write_fade_time` issues zero uniform writes on settled frames while still
    /// syncing newly created/swapped `SymbolDepth` attachments automatically.
    pub(super) fn effective_shader_time_ms(&self) -> f32 {
        if self.is_fading() {
            self.clock_ms
        } else {
            // +4.0 ms guarantees the shader clock clears any 4 ms `start_ticks` step + rounding.
            self.fade_until_ms + 4.0
        }
    }

    #[cfg(test)]
    pub(super) fn begin(&mut self, now: Duration) {
        self.begin_with_duration(now, Duration::from_millis(160));
    }

    pub(super) fn begin_with_duration(&mut self, now: Duration, fade_duration: Duration) {
        self.set_fade_duration(fade_duration);
        self.advance_clock(now);
        self.frame = self.frame.wrapping_add(1);
        self.repeats.begin();
        let retention = Duration::from_secs(1).max(fade_duration);
        self.states.retain(|_, layer_states| {
            layer_states.retain(|_, states| {
                states.retain(|state| now.saturating_sub(state.last_seen) < retention);
                !states.is_empty()
            });
            !layer_states.is_empty()
        });
    }

    pub(super) fn was_visible(&self, layer: &SymbolLayerData, feature: &Feature) -> bool {
        let Some(layer_states) = self.states.get(layer.style_layer_id.as_str()) else {
            return false;
        };
        layer_states
            .get(&feature_hash(feature))
            .and_then(|states| {
                states
                    .iter()
                    .find(|state| matches(state, layer.coords, feature))
            })
            .is_some_and(|state| state.target.iter().any(|v| *v))
    }

    /// Whether any visible or actively fading symbol state for `style_layer_id` could match a
    /// feature inside `tile` (including tile margin padding), allowing horizon/off-screen tiles
    /// without active fades to skip per-feature projection completely.
    pub(super) fn tile_has_active_state(
        &self,
        style_layer_id: &str,
        tile: WorldTileCoords,
    ) -> bool {
        let Some(layer_states) = self.states.get(style_layer_id) else {
            return false;
        };
        let scale = 2_f64.powi(i32::from(u8::from(tile.z)));
        let center_x = (f64::from(tile.x) + 0.5) / scale;
        let center_y = (f64::from(tile.y) + 0.5) / scale;
        let half_span = 0.76 / scale;
        layer_states.values().flatten().any(|state| {
            if !state.target[0]
                && !state.target[1]
                && state.ramp[0].is_hidden()
                && state.ramp[1].is_hidden()
            {
                return false;
            }
            if state.claimed_by == tile {
                return true;
            }
            let dx = (state.position[0] - center_x + 0.5).rem_euclid(1.0) - 0.5;
            let dy = state.position[1] - center_y;
            dx.abs() <= half_span && dy.abs() <= half_span
        })
    }

    /// Whether another tile's copy of the label already took it this frame, so this copy is
    /// neither drawn nor found by a query. Asked before [`Self::fade_ramps`] takes it.
    pub(super) fn shown_elsewhere(&self, layer: &SymbolLayerData, feature: &Feature) -> bool {
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
                        matches(state, layer.coords, feature)
                            && !(state.claimed == frame && state.claimed_by == layer.coords)
                    })
                    .is_some_and(|state| state.claimed == frame)
            })
    }

    pub(super) fn fade_ramps(
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
            matches(state, layer.coords, feature)
                && !(state.claimed == frame && state.claimed_by == layer.coords)
        });
        let Some(index) = existing.or_else(|| {
            any_target.then(|| {
                let ramp = target.map(|visible| {
                    let tgt = if visible { 1.0 } else { 0.0 };
                    FadeRamp {
                        start: if immediate { tgt } else { 0.0 },
                        target: tgt,
                        start_time_ms: if visible { now_ms } else { 0.0 },
                    }
                });
                states.push(State {
                    text: feature.str.clone(),
                    id: feature.data.id,
                    position: position(layer.coords, feature),
                    zoom: u8::from(layer.coords.z),
                    ramp,
                    target,
                    last_seen: self.now,
                    claimed: frame.wrapping_sub(1),
                    claimed_by: layer.coords,
                    anchor: None,
                });
                states.len() - 1
            })
        }) else {
            return [FadeRamp::default(); 2];
        };
        let state = &mut states[index];
        // A label repeated across tiles is drawn once, so a parent and child that are both
        // visible do not double up.
        if state.claimed == frame {
            return [FadeRamp::default(); 2];
        }
        for (i, &visible) in target.iter().enumerate() {
            let new_target = if visible { 1.0 } else { 0.0 };
            if immediate {
                state.ramp[i] = if visible {
                    FadeRamp {
                        start: 1.0,
                        target: 1.0,
                        start_time_ms: now_ms,
                    }
                } else {
                    FadeRamp::default()
                };
                continue;
            }
            let current = state.ramp[i].at(now_ms, fade_duration_ms);
            if state.target[i] != visible {
                state.ramp[i] = FadeRamp {
                    start: current,
                    target: new_target,
                    start_time_ms: now_ms,
                };
            } else if !visible && current <= 0.0 {
                state.ramp[i] = FadeRamp::default();
            }
            if (current - new_target).abs() > 1e-4 {
                self.fade_until_ms = self
                    .fade_until_ms
                    .max(state.ramp[i].start_time_ms + fade_duration_ms);
            }
        }
        state.target = target;
        state.last_seen = self.now;
        state.claimed = frame;
        state.claimed_by = layer.coords;
        state.position = position(layer.coords, feature);
        state.zoom = u8::from(layer.coords.z);
        state.ramp
    }

    #[cfg(test)]
    pub(super) fn opacity(
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
    pub(super) fn previous_anchor(
        &self,
        layer: &SymbolLayerData,
        feature: &Feature,
    ) -> Option<usize> {
        self.states
            .get(layer.style_layer_id.as_str())?
            .get(&feature_hash(feature))?
            .iter()
            .find(|state| matches(state, layer.coords, feature))?
            .anchor
    }

    /// Records the anchor this frame placed the label's text with, on the state
    /// [`Self::fade_ramps`] took for it. A label not placed keeps its anchor while it fades out,
    /// as GL JS keeps the offsets of symbols that are not yet hidden.
    pub(super) fn remember_anchor(
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
                    state.claimed == frame
                        && state.claimed_by == layer.coords
                        && matches(state, layer.coords, feature)
                })
            })
        {
            if anchor.is_some() || state.ramp[0].at(now_ms, fade_duration_ms) <= 0.0 {
                state.anchor = anchor;
            }
        }
    }
}

fn feature_hash(feature: &Feature) -> u64 {
    let mut hasher = FxHasher::default();
    feature.str.hash(&mut hasher);
    feature.data.id.hash(&mut hasher);
    hasher.finish()
}

fn position(tile: WorldTileCoords, feature: &Feature) -> [f64; 2] {
    let scale = 2_f64.powi(i32::from(u8::from(tile.z)));
    [
        (f64::from(tile.x) + f64::from(feature.text_anchor.x) / 4096.0) / scale,
        (f64::from(tile.y) + f64::from(feature.text_anchor.y) / 4096.0) / scale,
    ]
}

fn matches(state: &State, tile: WorldTileCoords, feature: &Feature) -> bool {
    if state.id != feature.data.id || state.text != feature.str {
        return false;
    }
    let point = position(tile, feature);
    let tolerance = 8.0 / (4096.0 * 2_f64.powi(i32::from(state.zoom.min(u8::from(tile.z)))));
    let dx = (point[0] - state.position[0] + 0.5).rem_euclid(1.0) - 0.5;
    dx.abs() <= tolerance && (point[1] - state.position[1]).abs() <= tolerance
}

#[cfg(test)]
mod tests;

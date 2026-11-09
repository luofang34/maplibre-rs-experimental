//! Placement history follows a symbol across tile replacement, with bounded fades.
use std::{
    hash::{Hash, Hasher},
    time::Duration,
};

use rustc_hash::{FxHashMap, FxHasher};

use crate::{
    coords::WorldTileCoords,
    sdf::{Feature, SymbolLayerData},
};

#[derive(Clone)]
struct State {
    text: String,
    id: Option<u64>,
    position: [f64; 2],
    zoom: u8,
    opacity: [f32; 2],
    target: [bool; 2],
    last_seen: Duration,
    claimed: u64,
    claimed_by: WorldTileCoords,
    /// The variable anchor the text was last placed with.
    anchor: Option<usize>,
}
#[derive(Default)]
pub(super) struct PlacementHistory {
    states: FxHashMap<String, FxHashMap<u64, Vec<State>>>,
    now: Duration,
    frame: u64,
    pub(super) fading: bool,
    /// Where the repeats of labels along lines were placed on screen.
    pub(super) repeats: super::repeat_spacing::RepeatSpacing,
}
impl PlacementHistory {
    pub(super) fn begin(&mut self, now: Duration) {
        self.now = now;
        self.frame = self.frame.wrapping_add(1);
        self.fading = false;
        self.repeats.begin();
        self.states.retain(|_, layer_states| {
            layer_states.retain(|_, states| {
                states.retain(|state| now.saturating_sub(state.last_seen) < Duration::from_secs(1));
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
    /// Whether another tile's copy of the label already took it this frame, so this copy is
    /// neither drawn nor found by a query. Asked before [`Self::opacity`] takes it.
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

    pub(super) fn opacity(
        &mut self,
        layer: &SymbolLayerData,
        feature: &Feature,
        target: [bool; 2],
    ) -> [f32; 2] {
        let frame = self.frame;
        let any_target = target[0] || target[1];
        let hash = feature_hash(feature);
        if !any_target {
            let Some(layer_states) = self.states.get(layer.style_layer_id.as_str()) else {
                return [0.0; 2];
            };
            if !layer_states.contains_key(&hash) {
                return [0.0; 2];
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
                states.push(State {
                    text: feature.str.clone(),
                    id: feature.data.id,
                    position: position(layer.coords, feature),
                    zoom: u8::from(layer.coords.z),
                    opacity: [0.0; 2],
                    target,
                    last_seen: self.now,
                    claimed: frame.wrapping_sub(1),
                    claimed_by: layer.coords,
                    anchor: None,
                });
                states.len() - 1
            })
        }) else {
            return [0.0; 2];
        };
        let state = &mut states[index];
        // A label repeated across tiles is drawn once, so a parent and child that are both
        // visible do not double up.
        if state.claimed == frame {
            return [0.0; 2];
        }
        let step = (self.now.saturating_sub(state.last_seen).as_secs_f32() / 0.16).min(1.0);
        for (i, visible) in target.iter().enumerate() {
            state.opacity[i] =
                (state.opacity[i] + if state.target[i] { step } else { -step }).clamp(0.0, 1.0);
            self.fading |= state.opacity[i] != if *visible { 1.0 } else { 0.0 };
        }
        state.target = target;
        state.last_seen = self.now;
        state.claimed = frame;
        state.claimed_by = layer.coords;
        state.position = position(layer.coords, feature);
        state.zoom = u8::from(layer.coords.z);
        state.opacity
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
    /// [`Self::opacity`] took for it. A label not placed keeps its anchor while it fades out,
    /// as GL JS keeps the offsets of symbols that are not yet hidden.
    pub(super) fn remember_anchor(
        &mut self,
        layer: &SymbolLayerData,
        feature: &Feature,
        anchor: Option<usize>,
    ) {
        let frame = self.frame;
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
            if anchor.is_some() || state.opacity[0] == 0.0 {
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

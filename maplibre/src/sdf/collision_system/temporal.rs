//! Placement history follows a symbol across tile replacement, with bounded fades.
use std::{collections::HashMap, time::Duration};

use crate::{
    coords::WorldTileCoords,
    sdf::{Feature, SymbolLayerData},
};

#[derive(Clone, Hash, PartialEq, Eq)]
struct Key {
    layer: String,
    text: String,
    id: Option<u64>,
}
#[derive(Clone)]
struct State {
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
    states: HashMap<Key, Vec<State>>,
    now: Duration,
    frame: u64,
    pub(super) fading: bool,
}
impl PlacementHistory {
    pub(super) fn begin(&mut self, now: Duration) {
        self.now = now;
        self.frame = self.frame.wrapping_add(1);
        self.fading = false;
        self.states.retain(|_, states| {
            states.retain(|state| now.saturating_sub(state.last_seen) < Duration::from_secs(1));
            !states.is_empty()
        });
    }
    pub(super) fn was_visible(&self, layer: &SymbolLayerData, feature: &Feature) -> bool {
        self.states
            .get(&key(layer, feature))
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
        let frame = self.frame;
        self.states.get(&key(layer, feature)).is_some_and(|states| {
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
        let states = self.states.entry(key(layer, feature)).or_default();
        // A state a same-tile feature already took this frame belongs to that feature, so a
        // second feature of the tile with the same key is tracked separately.
        let existing = states.iter().position(|state| {
            matches(state, layer.coords, feature)
                && !(state.claimed == frame && state.claimed_by == layer.coords)
        });
        let index = existing.unwrap_or_else(|| {
            states.push(State {
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
        });
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
            .get(&key(layer, feature))?
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
            .get_mut(&key(layer, feature))
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

fn key(layer: &SymbolLayerData, feature: &Feature) -> Key {
    Key {
        layer: layer.style_layer_id.clone(),
        text: feature.str.clone(),
        id: feature.data.id,
    }
}
fn position(tile: WorldTileCoords, feature: &Feature) -> [f64; 2] {
    let scale = 2_f64.powi(i32::from(u8::from(tile.z)));
    [
        (f64::from(tile.x) + f64::from(feature.text_anchor.x) / 4096.0) / scale,
        (f64::from(tile.y) + f64::from(feature.text_anchor.y) / 4096.0) / scale,
    ]
}
fn matches(state: &State, tile: WorldTileCoords, feature: &Feature) -> bool {
    let point = position(tile, feature);
    let tolerance = 8.0 / (4096.0 * 2_f64.powi(i32::from(state.zoom.min(u8::from(tile.z)))));
    let dx = (point[0] - state.position[0] + 0.5).rem_euclid(1.0) - 0.5;
    dx.abs() <= tolerance && (point[1] - state.position[1]).abs() <= tolerance
}

#[cfg(test)]
mod tests;

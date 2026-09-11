//! Placement history follows a symbol across tile replacement, with bounded fades.
use crate::{
    coords::WorldTileCoords,
    sdf::{Feature, SymbolLayerData},
};
use std::{collections::HashMap, time::Duration};

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
    pub(super) fn opacity(
        &mut self,
        layer: &SymbolLayerData,
        feature: &Feature,
        target: [bool; 2],
    ) -> [f32; 2] {
        let frame = self.frame;
        let states = self.states.entry(key(layer, feature)).or_default();
        let existing = states
            .iter()
            .position(|state| matches(state, layer.coords, feature));
        let index = existing.unwrap_or_else(|| {
            states.push(State {
                position: position(layer.coords, feature),
                zoom: u8::from(layer.coords.z),
                opacity: [0.0; 2],
                target,
                last_seen: self.now,
                claimed: frame.wrapping_sub(1),
            });
            states.len() - 1
        });
        let state = &mut states[index];
        // One conceptual label is drawn once even if both a parent and child are visible.
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
        state.position = position(layer.coords, feature);
        state.zoom = u8::from(layer.coords.z);
        state.opacity
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

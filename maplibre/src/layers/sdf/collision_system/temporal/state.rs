//! Per-symbol fade and placement tracking state across tile replacements.

use std::{
    hash::{Hash, Hasher},
    time::Duration,
};

use rustc_hash::FxHasher;

use super::FadeRamp;
use crate::{
    coords::WorldTileCoords,
    sdf::{Feature, SymbolLayerData},
};

#[derive(Clone)]
pub(super) struct State {
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

fn initial_ramp(visible: bool, now_ms: f32, immediate: bool) -> FadeRamp {
    let tgt = if visible { 1.0 } else { 0.0 };
    FadeRamp {
        start: if immediate { tgt } else { 0.0 },
        target: tgt,
        start_time_ms: if visible { now_ms } else { 0.0 },
    }
}

fn immediate_ramp(visible: bool, now_ms: f32) -> FadeRamp {
    if visible {
        FadeRamp {
            start: 1.0,
            target: 1.0,
            start_time_ms: now_ms,
        }
    } else {
        FadeRamp::default()
    }
}

fn wrapped_dx(a: f64, b: f64) -> f64 {
    (a - b + 0.5).rem_euclid(1.0) - 0.5
}

pub(super) fn tile_scale(z: u8) -> f64 {
    2_f64.powi(i32::from(z))
}

impl State {
    pub(super) fn new_for_target(
        layer: &SymbolLayerData,
        feature: &Feature,
        target: [bool; 2],
        now: Duration,
        now_ms: f32,
        immediate: bool,
        claimed: u64,
    ) -> Self {
        let ramp = target.map(|visible| initial_ramp(visible, now_ms, immediate));
        Self {
            text: feature.str.clone(),
            id: feature.data.id,
            position: position(layer.coords, feature),
            zoom: u8::from(layer.coords.z),
            ramp,
            target,
            last_seen: now,
            claimed,
            claimed_by: layer.coords,
            anchor: None,
        }
    }

    #[inline]
    pub(super) fn is_target_visible(&self) -> bool {
        self.target[0] || self.target[1]
    }

    #[inline]
    pub(super) fn is_claimed_in_frame(&self, frame: u64) -> bool {
        self.claimed == frame
    }

    #[inline]
    pub(super) fn is_claimed_by_tile(&self, frame: u64, tile: WorldTileCoords) -> bool {
        self.claimed == frame && self.claimed_by == tile
    }

    #[inline]
    pub(super) fn ramps(&self) -> [FadeRamp; 2] {
        self.ramp
    }

    #[inline]
    pub(super) fn anchor(&self) -> Option<usize> {
        self.anchor
    }

    pub(super) fn update_anchor(
        &mut self,
        anchor: Option<usize>,
        now_ms: f32,
        fade_duration_ms: f32,
    ) {
        if anchor.is_some() || self.ramp[0].at(now_ms, fade_duration_ms) <= 0.0 {
            self.anchor = anchor;
        }
    }

    pub(super) fn touch_and_is_retained(
        &mut self,
        prev_frame: u64,
        touched: Duration,
        now: Duration,
        retention: Duration,
    ) -> bool {
        if self.claimed == prev_frame && touched > self.last_seen {
            self.last_seen = touched;
        }
        now.saturating_sub(self.last_seen) < retention
    }

    pub(super) fn claim_for_frame(
        &mut self,
        target: [bool; 2],
        now: Duration,
        frame: u64,
        tile: WorldTileCoords,
        feature: &Feature,
    ) {
        self.target = target;
        self.last_seen = now;
        self.claimed = frame;
        self.claimed_by = tile;
        self.position = position(tile, feature);
        self.zoom = u8::from(tile.z);
    }

    pub(super) fn matches(&self, tile: WorldTileCoords, feature: &Feature) -> bool {
        if self.id != feature.data.id || self.text != feature.str {
            return false;
        }
        let point = position(tile, feature);
        let tolerance = 8.0 / (4096.0 * tile_scale(self.zoom.min(u8::from(tile.z))));
        let dx = wrapped_dx(point[0], self.position[0]);
        dx.abs() <= tolerance && (point[1] - self.position[1]).abs() <= tolerance
    }

    #[inline]
    pub(super) fn is_active(&self) -> bool {
        self.target[0] || self.target[1] || !self.ramp[0].is_hidden() || !self.ramp[1].is_hidden()
    }

    #[inline]
    pub(super) fn overlaps_tile(
        &self,
        tile: WorldTileCoords,
        center_x: f64,
        center_y: f64,
        half_span: f64,
    ) -> bool {
        if self.claimed_by == tile {
            return true;
        }
        let dx = wrapped_dx(self.position[0], center_x);
        let dy = self.position[1] - center_y;
        dx.abs() <= half_span && dy.abs() <= half_span
    }

    pub(super) fn rebase_settled(&mut self) {
        for ramp in &mut self.ramp {
            ramp.start = ramp.target;
            ramp.start_time_ms = 0.0;
        }
    }

    pub(super) fn update_ramps(
        &mut self,
        target: [bool; 2],
        now_ms: f32,
        fade_duration_ms: f32,
    ) -> f32 {
        let [ramp0, ramp1] = &mut self.ramp;
        let [vis0, vis1] = target;
        let [prev0, prev1] = self.target;
        let mut max_fade_until = 0.0_f32;
        for (ramp_slot, visible, prev_visible) in [(ramp0, vis0, prev0), (ramp1, vis1, prev1)] {
            max_fade_until = max_fade_until.max(update_single_ramp(
                ramp_slot,
                visible,
                prev_visible,
                now_ms,
                fade_duration_ms,
            ));
        }
        max_fade_until
    }
}

fn update_single_ramp(
    ramp_slot: &mut FadeRamp,
    visible: bool,
    prev_visible: bool,
    now_ms: f32,
    fade_duration_ms: f32,
) -> f32 {
    let new_target = if visible { 1.0 } else { 0.0 };
    if fade_duration_ms <= 0.0 {
        *ramp_slot = immediate_ramp(visible, now_ms);
        return 0.0;
    }
    let current = ramp_slot.at(now_ms, fade_duration_ms);
    if prev_visible != visible {
        *ramp_slot = FadeRamp {
            start: current,
            target: new_target,
            start_time_ms: now_ms,
        };
    } else if !visible && current <= 0.0 {
        *ramp_slot = FadeRamp::default();
    }
    if (current - new_target).abs() > 1e-4 {
        ramp_slot.start_time_ms + fade_duration_ms
    } else {
        0.0
    }
}

pub(super) fn feature_hash(feature: &Feature) -> u64 {
    let mut hasher = FxHasher::default();
    feature.str.hash(&mut hasher);
    feature.data.id.hash(&mut hasher);
    hasher.finish()
}

pub(super) fn position(tile: WorldTileCoords, feature: &Feature) -> [f64; 2] {
    let scale = tile_scale(u8::from(tile.z));
    [
        (f64::from(tile.x) + f64::from(feature.text_anchor.x) / 4096.0) / scale,
        (f64::from(tile.y) + f64::from(feature.text_anchor.y) / 4096.0) / scale,
    ]
}

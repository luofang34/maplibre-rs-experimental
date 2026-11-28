//! Zoom-history tracking, integer-zoom cross-fade parameters, and `line-floorwidth` evaluation.

use std::time::Duration;

use bytemuck_derive::{Pod, Zeroable};

use super::pixels::normalize_pattern;
use crate::style::{
    layer::LinePaint,
    property::{NumberList, StyleProperty},
    spec::SpecPropertyKind,
};

/// Duration of integer-zoom dash cross-fades, matching MapLibre GL JS `defaultFadeDuration`.
pub(super) const DEFAULT_FADE_DURATION: Duration = Duration::from_millis(300);

/// Cross-fade blend parameters across integer zoom boundaries.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct CrossfadeParameters {
    /// Tile-unit scale multiplier applied to the outgoing `from` pattern (`2.0` zooming in, `0.5` out).
    pub from_scale: f32,
    /// Tile-unit scale multiplier applied to the incoming `to` pattern (`1.0`).
    pub to_scale: f32,
    /// Blend weight in `[0.0, 1.0]` interpolating from `from` (`0.0`) toward `to` (`1.0`).
    pub mix: f32,
    /// Normalized fade completion progress in `[0.0, 1.0]`.
    pub fade_t: f32,
}

/// Tracks view zoom history to drive GL JS-parity integer-zoom cross-fades.
#[derive(Clone, Copy, Debug)]
pub(super) struct ZoomHistory {
    last_zoom: f64,
    last_floor_zoom: f64,
    last_integer_zoom: f64,
    last_integer_zoom_time: Option<Duration>,
    first: bool,
}

impl Default for ZoomHistory {
    fn default() -> Self {
        Self {
            last_zoom: 0.0,
            last_floor_zoom: 0.0,
            last_integer_zoom: 0.0,
            last_integer_zoom_time: None,
            first: true,
        }
    }
}

impl ZoomHistory {
    /// Records the current `zoom` at frame clock `now` (or `None` when settled immediately).
    pub(super) fn update(&mut self, zoom: f64, now: Option<Duration>) {
        let floor_z = zoom.floor();
        if self.first {
            self.first = false;
            self.last_integer_zoom = floor_z;
            self.last_integer_zoom_time = None;
            self.last_zoom = zoom;
            self.last_floor_zoom = floor_z;
            return;
        }
        if self.last_floor_zoom > floor_z {
            self.last_integer_zoom = floor_z + 1.0;
            self.last_integer_zoom_time = now;
        } else if self.last_floor_zoom < floor_z {
            self.last_integer_zoom = floor_z;
            self.last_integer_zoom_time = now;
        }
        if zoom != self.last_zoom {
            self.last_zoom = zoom;
            self.last_floor_zoom = floor_z;
        }
    }

    /// Returns `true` when the latest integer boundary was crossed while zooming in.
    pub(super) fn zooming_in(&self, zoom: f64) -> bool {
        zoom > self.last_integer_zoom
    }

    /// Computes [`CrossfadeParameters`] at `zoom`, `now`, and fade `duration`.
    pub(super) fn crossfade(
        &self,
        zoom: f64,
        now: Option<Duration>,
        duration: Duration,
    ) -> CrossfadeParameters {
        let fraction = (zoom - zoom.floor()) as f32;
        let fade_t = match (now, self.last_integer_zoom_time) {
            _ if duration.is_zero() => 1.0,
            (Some(now), Some(last)) => (now.saturating_sub(last).as_secs_f64()
                / duration.as_secs_f64())
            .clamp(0.0, 1.0) as f32,
            _ => 1.0,
        };
        if self.zooming_in(zoom) {
            CrossfadeParameters {
                from_scale: 2.0,
                to_scale: 1.0,
                mix: (fraction + (1.0 - fraction) * fade_t).clamp(0.0, 1.0),
                fade_t,
            }
        } else {
            CrossfadeParameters {
                from_scale: 0.5,
                to_scale: 1.0,
                mix: (1.0 - (1.0 - fade_t) * fraction).clamp(0.0, 1.0),
                fade_t,
            }
        }
    }
}

/// 48-byte uniform payload bound at `@group(1) @binding(2)` for line dashes, gradients, and patterns.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub(super) struct DashUniforms {
    /// `[to_period, mode, round_or_display_w, dash_scale_or_display_h]`.
    pub params: [f32; 4],
    /// `[from_period, from_scale, to_scale, mix]`.
    pub fade: [f32; 4],
    /// `[floor_width, dual_rows, 0.0, 0.0]`.
    pub extra: [f32; 4],
}

/// Parses `line-dasharray` using the v8 spec so legacy stop tables use stepped (`interval`) lookup.
pub(super) fn parse_dasharray(value: &serde_json::Value) -> StyleProperty<NumberList> {
    StyleProperty::<NumberList>::parse_for_layer_property(
        "line",
        SpecPropertyKind::Paint,
        "line-dasharray",
        value,
    )
}

/// Evaluates the normalized `(from, to)` dash patterns across the active integer-zoom step.
pub(super) fn evaluate_dash_patterns(
    paint: &LinePaint,
    floor_z: f64,
    zooming_in: bool,
) -> (Vec<f64>, Vec<f64>) {
    let Some(value) = paint.line_dasharray.as_ref() else {
        return (Vec::new(), Vec::new());
    };
    let prop = parse_dasharray(value);
    if prop.is_zoom_constant() {
        let pattern = prop
            .evaluate_at_zoom(floor_z)
            .map_or_else(Vec::new, |values| normalize_pattern(values.0));
        return (pattern.clone(), pattern);
    }
    let from_z = if zooming_in {
        floor_z - 1.0
    } else {
        floor_z + 1.0
    };
    let from_pattern = prop
        .evaluate_at_zoom(from_z)
        .map_or_else(Vec::new, |values| normalize_pattern(values.0));
    let to_pattern = prop
        .evaluate_at_zoom(floor_z)
        .map_or_else(Vec::new, |values| normalize_pattern(values.0));
    (from_pattern, to_pattern)
}

/// Evaluates `line-floorwidth` at `zoom.floor()`, or returns `-1.0` when `line-width` is data-driven.
pub(super) fn eval_floor_width(paint: &LinePaint, zoom: f64) -> f32 {
    match paint.line_width.as_ref() {
        None => 1.0,
        Some(prop) if !prop.is_feature_constant() => -1.0,
        Some(prop) => prop
            .evaluate_at_zoom(zoom.floor())
            .filter(|width| width.is_finite())
            .map_or(1.0, |width| width.max(0.0)),
    }
}

/// Computes the fragment shader dash U delta per tile unit for `paint` at `zoom`.
#[cfg(test)]
pub(super) fn dash_uv_per_tile_unit(
    paint: &LinePaint,
    zoom: f64,
    tile_z: u8,
    texture_zoom: Option<f64>,
) -> Option<f32> {
    use super::pixels::dash_scale;
    use crate::coords::{WorldTileCoords, Zoom, ZoomLevel};

    let coords = WorldTileCoords {
        x: 0,
        y: 0,
        z: ZoomLevel::new(tile_z),
    };
    let (_, to_pattern) = evaluate_dash_patterns(paint, zoom.floor(), true);
    let period = to_pattern.iter().sum::<f64>() as f32;
    if period <= 0.0 {
        return None;
    }
    let floor_width = eval_floor_width(paint, zoom.floor());
    let (line_units_per_pixel, line_width_scale) = match texture_zoom {
        Some(tex_z) => (
            8.0 * Zoom::new(tex_z).scale_to_zoom_level(coords.z) as f32,
            2.0_f64.powf(tex_z - zoom) as f32,
        ),
        None => (
            8.0 * Zoom::new(zoom).scale_to_zoom_level(coords.z) as f32,
            1.0,
        ),
    };
    let floor_width_px = floor_width * line_width_scale;
    let dash_scale_frac = dash_scale(zoom);
    Some(1.0 / (line_units_per_pixel * floor_width_px * period * dash_scale_frac))
}

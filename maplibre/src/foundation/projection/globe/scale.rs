//! Style zoom on a globe whose camera may look at any point, the poles included.
//!
//! The camera's physical scale is the globe's radius in screen pixels. Style zoom, which
//! `["zoom"]` expressions, layer zoom ranges and the integer layout zoom read, stays what the
//! Mercator world defines wherever that world reaches: the zoom of the world whose parallel
//! at the center's latitude has the globe's circumference. Past the Mercator world's last
//! latitude the parallel is taken at that latitude, so near a pole the style zoom follows the
//! physical scale without running off to minus infinity as the parallel shrinks to a point,
//! and a camera crossing the pole sees it change continuously and symmetrically.

use std::f64::consts::TAU;

use crate::coords::TILE_SIZE;

/// Latitude of the Mercator world's edge, beyond which the parallel stops shrinking.
pub const MERCATOR_LATITUDE_LIMIT: f64 = 85.051_128_779_806_59;

/// Cosine of the latitude the style zoom measures the parallel at.
fn parallel_scale(latitude_degrees: f64) -> f64 {
    latitude_degrees
        .abs()
        .min(MERCATOR_LATITUDE_LIMIT)
        .to_radians()
        .cos()
}

/// The style zoom of a globe drawn `radius_pixels` large around a center at `latitude_degrees`.
pub fn style_zoom(radius_pixels: f64, latitude_degrees: f64) -> f64 {
    (radius_pixels * TAU * parallel_scale(latitude_degrees) / TILE_SIZE).log2()
}

/// The globe's radius in pixels that style zoom `zoom` stands for at `latitude_degrees`.
pub fn radius_pixels(zoom: f64, latitude_degrees: f64) -> f64 {
    TILE_SIZE * 2_f64.powf(zoom) / (TAU * parallel_scale(latitude_degrees))
}

#[cfg(test)]
mod tests;

//! Map projection definitions and coordinate conversion.

use std::f64::consts::PI;

use cgmath::Vector3;
use serde::{Deserialize, Serialize};

use crate::coords::EXTENT;

/// A projection selected by a style.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProjectionType {
    /// Render Web Mercator tiles on a plane.
    #[default]
    Mercator,
    /// Render Web Mercator tiles on a globe.
    Globe,
}

/// The root-level projection section of a style document.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProjectionSpecification {
    /// Projection used to display the map.
    #[serde(rename = "type")]
    pub projection_type: ProjectionType,
}

/// Projects tile-local Web Mercator coordinates onto a unit sphere.
///
/// Tile coordinates use the crate's [`EXTENT`] and XYZ addressing. The returned axes match
/// MapLibre GL JS: positive Y points north and `[0°, 0°]` maps to positive Z.
pub fn project_tile_coordinates_to_unit_sphere(
    tile_x: u32,
    tile_y: u32,
    zoom: u8,
    in_tile_x: f64,
    in_tile_y: f64,
) -> Vector3<f64> {
    let tile_count = 2_f64.powi(i32::from(zoom));
    let mercator_x = (f64::from(tile_x) + in_tile_x / EXTENT) / tile_count;
    let mercator_y = (f64::from(tile_y) + in_tile_y / EXTENT) / tile_count;
    let longitude = mercator_x * PI * 2.0 + PI;

    // This rational form mirrors the globe shader and avoids precision loss from subtracting
    // PI / 2 after evaluating the inverse Web Mercator Gudermannian.
    let tangent_half_latitude = (PI - mercator_y * PI * 2.0).exp();
    let tangent_half_latitude_squared = tangent_half_latitude * tangent_half_latitude;
    let denominator = tangent_half_latitude_squared + 1.0;
    let sin_latitude = (tangent_half_latitude_squared - 1.0) / denominator;
    let cos_latitude = (2.0 * tangent_half_latitude) / denominator;

    Vector3::new(
        longitude.sin() * cos_latitude,
        sin_latitude,
        longitude.cos() * cos_latitude,
    )
}

#[cfg(test)]
mod tests;

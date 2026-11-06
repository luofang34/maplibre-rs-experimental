//! Globe tile distance and convex-volume calculations used by tile covering.

use cgmath::{InnerSpace, Point2, Vector3, Vector4};
use thiserror::Error;

use super::project_tile_coordinates_to_unit_sphere;
use crate::{
    coords::{TileCoords, EXTENT},
    projection::body::Body,
};

mod cap;
pub(crate) use cap::{closest_mercator_data_point_on_tile, closest_unit_sphere_point_on_tile};
pub use cap::{tile_spherical_cap, TileSphericalCap};

/// Minimum and maximum elevation included in a tile bounding volume.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TileElevationRange {
    /// Minimum elevation in metres.
    pub min_meters: f64,
    /// Maximum elevation in metres.
    pub max_meters: f64,
}

/// Elevation bounds of individual tiles for culling volumes.
pub trait TileElevationProvider {
    /// Bounds of one canonical tile.
    fn elevation_range(&self, tile: TileCoords) -> TileElevationRange;
}

impl TileElevationProvider for TileElevationRange {
    fn elevation_range(&self, _tile: TileCoords) -> TileElevationRange {
        *self
    }
}

/// Convex volume approximating a curved globe tile and its elevation range.
#[derive(Clone, Debug, PartialEq)]
pub struct GlobeTileBoundingVolume {
    /// Points defining the convex shape.
    pub points: Vec<Vector3<f64>>,
    /// Inward-facing half-space planes containing the volume.
    pub planes: [Vector4<f64>; 6],
    /// Minimum corner of the tile surface's rejection AABB.
    pub min: Vector3<f64>,
    /// Maximum corner of the tile surface's rejection AABB.
    pub max: Vector3<f64>,
}

/// Failure while constructing a globe tile bounding volume.
#[derive(Clone, Copy, Debug, Error, PartialEq)]
pub enum GlobeTileBoundsError {
    /// Elevation values must be finite.
    #[error("tile elevation range must be finite, got {min_meters}..{max_meters} metres")]
    InvalidElevation {
        /// Invalid minimum elevation.
        min_meters: f64,
        /// Invalid maximum elevation.
        max_meters: f64,
    },
    /// The tile's boundary planes do not have a unique intersection.
    #[error("globe tile planes are degenerate for z={z}, x={x}, y={y}")]
    DegeneratePlanes {
        /// Tile zoom.
        z: u8,
        /// Tile X coordinate.
        x: u32,
        /// Tile Y coordinate.
        y: u32,
    },
}

/// Returns the nearest wrapped, pole-aware Mercator distance from a point to a tile.
pub fn distance_to_tile_2d(point: Point2<f64>, tile: TileCoords) -> f64 {
    let scale = 2_f64.powi(i32::from(u8::from(tile.z)));
    let size = 1.0 / scale;
    let corner_x = f64::from(tile.x) / scale;
    let corner_y = f64::from(tile.y) / scale;
    let original = distance_to_tile_wrap_x(point, corner_x, corner_y, size);
    let north = distance_to_tile_wrap_x(point, corner_x + 0.5, -corner_y - size, size);
    let south = distance_to_tile_wrap_x(point, corner_x + 0.5, 2.0 - corner_y - size, size);
    original.min(north).min(south)
}

/// Selects the wrap nearest to a Mercator center so tiles persist across the antimeridian.
pub fn nearest_tile_wrap(center_x: f64, tile: TileCoords) -> i8 {
    let scale = 2_f64.powi(i32::from(u8::from(tile.z)));
    let size = 1.0 / scale;
    let tile_x = f64::from(tile.x) / scale;
    let current = distance_to_interval(center_x, tile_x, size);
    let left = distance_to_interval(center_x, tile_x - 1.0, size);
    let right = distance_to_interval(center_x, tile_x + 1.0, size);
    let smallest = current.min(left).min(right);
    if smallest == right {
        1
    } else if smallest == left {
        -1
    } else {
        0
    }
}

/// Computes the convex bounding volume for a globe tile and elevation range.
pub fn globe_tile_bounding_volume(
    tile: TileCoords,
    elevation: TileElevationRange,
    body: Body,
) -> Result<GlobeTileBoundingVolume, GlobeTileBoundsError> {
    validate_elevation(elevation)?;
    let min_radius = body.unit_radius_at(elevation.min_meters.min(0.0));
    let max_radius = body.unit_radius_at(elevation.max_meters.max(0.0));
    match u8::from(tile.z) {
        0 => Ok(aabb_volume(
            Vector3::new(-max_radius, -max_radius, -max_radius),
            Vector3::new(max_radius, max_radius, max_radius),
        )),
        1 => Ok(zoom_one_volume(tile, max_radius)),
        _ => curved_tile_volume(tile, min_radius, max_radius),
    }
}

pub(crate) fn validate_elevation(
    elevation: TileElevationRange,
) -> Result<(), GlobeTileBoundsError> {
    if !elevation.min_meters.is_finite() || !elevation.max_meters.is_finite() {
        return Err(GlobeTileBoundsError::InvalidElevation {
            min_meters: elevation.min_meters,
            max_meters: elevation.max_meters,
        });
    }
    Ok(())
}

fn distance_to_interval(point: f64, start: f64, size: f64) -> f64 {
    let delta = point - start;
    if delta < 0.0 {
        -delta
    } else {
        (delta - size).max(0.0)
    }
}

fn distance_to_tile_wrap_x(point: Point2<f64>, corner_x: f64, corner_y: f64, size: f64) -> f64 {
    let delta_x = point.x - corner_x;
    let distance_x = if delta_x < 0.0 {
        (-delta_x).min(1.0 + delta_x - size)
    } else if delta_x > size {
        (delta_x - size).max(0.0).min(1.0 - delta_x)
    } else {
        0.0
    };
    distance_x.max(distance_to_interval(point.y, corner_y, size))
}

fn zoom_one_volume(tile: TileCoords, radius: f64) -> GlobeTileBoundingVolume {
    let min = Vector3::new(
        if tile.x == 0 { -radius } else { 0.0 },
        if tile.y == 0 { 0.0 } else { -radius },
        -radius,
    );
    let max = Vector3::new(
        if tile.x == 0 { 0.0 } else { radius },
        if tile.y == 0 { radius } else { 0.0 },
        radius,
    );
    aabb_volume(min, max)
}

pub(crate) fn aabb_volume(min: Vector3<f64>, max: Vector3<f64>) -> GlobeTileBoundingVolume {
    let mut points = Vec::with_capacity(8);
    for index in 0..8 {
        points.push(Vector3::new(
            if index & 1 == 0 { min.x } else { max.x },
            if index & 2 == 0 { min.y } else { max.y },
            if index & 4 == 0 { min.z } else { max.z },
        ));
    }
    GlobeTileBoundingVolume {
        points,
        planes: [
            Vector4::new(-1.0, 0.0, 0.0, max.x),
            Vector4::new(1.0, 0.0, 0.0, -min.x),
            Vector4::new(0.0, -1.0, 0.0, max.y),
            Vector4::new(0.0, 1.0, 0.0, -min.y),
            Vector4::new(0.0, 0.0, -1.0, max.z),
            Vector4::new(0.0, 0.0, 1.0, -min.z),
        ],
        min,
        max,
    }
}

fn curved_tile_volume(
    tile: TileCoords,
    min_radius: f64,
    max_radius: f64,
) -> Result<GlobeTileBoundingVolume, GlobeTileBoundsError> {
    let corners = tile_corners(tile);
    let mut extremes: Vec<_> = corners.iter().map(|corner| *corner * max_radius).collect();
    if min_radius != max_radius {
        extremes.extend(corners.iter().map(|corner| *corner * min_radius));
    }
    extend_pole_extremes(&mut extremes, tile, min_radius, max_radius);
    let (min, max) = bounds(&extremes);
    let axes = tile_axes(tile);
    extend_curved_edge_extremes(&mut extremes, tile, axes.center, max_radius);
    let planes = tile_planes(&extremes, axes);
    let points = volume_points(tile, planes)?;
    Ok(GlobeTileBoundingVolume {
        points,
        planes,
        min,
        max,
    })
}

fn tile_corners(tile: TileCoords) -> [Vector3<f64>; 4] {
    let zoom = u8::from(tile.z);
    [
        project_tile_coordinates_to_unit_sphere(tile.x, tile.y, zoom, 0.0, 0.0),
        project_tile_coordinates_to_unit_sphere(tile.x, tile.y, zoom, EXTENT, 0.0),
        project_tile_coordinates_to_unit_sphere(tile.x, tile.y, zoom, EXTENT, EXTENT),
        project_tile_coordinates_to_unit_sphere(tile.x, tile.y, zoom, 0.0, EXTENT),
    ]
}

fn extend_pole_extremes(
    extremes: &mut Vec<Vector3<f64>>,
    tile: TileCoords,
    min_radius: f64,
    max_radius: f64,
) {
    let tile_count = 2_u32.pow(u32::from(u8::from(tile.z)));
    for (matches_pole, pole) in [
        (tile.y == 0, Vector3::unit_y()),
        (tile.y == tile_count - 1, -Vector3::unit_y()),
    ] {
        if matches_pole {
            extremes.push(pole * max_radius);
            if min_radius != max_radius {
                extremes.push(pole * min_radius);
            }
        }
    }
}

fn bounds(points: &[Vector3<f64>]) -> (Vector3<f64>, Vector3<f64>) {
    let mut min = Vector3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
    let mut max = Vector3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for point in points {
        min.x = min.x.min(point.x);
        min.y = min.y.min(point.y);
        min.z = min.z.min(point.z);
        max.x = max.x.max(point.x);
        max.y = max.y.max(point.y);
        max.z = max.z.max(point.z);
    }
    (min, max)
}

#[derive(Clone, Copy)]
struct TileAxes {
    center: Vector3<f64>,
    north: Vector3<f64>,
    east: Vector3<f64>,
    west: Vector3<f64>,
}

fn tile_axes(tile: TileCoords) -> TileAxes {
    let tile_count = 2_f64.powi(i32::from(u8::from(tile.z)));
    let lon_west = (f64::from(tile.x) / tile_count) * std::f64::consts::TAU + std::f64::consts::PI;
    let lon_center =
        ((f64::from(tile.x) + 0.5) / tile_count) * std::f64::consts::TAU + std::f64::consts::PI;
    let lon_east =
        (f64::from(tile.x + 1) / tile_count) * std::f64::consts::TAU + std::f64::consts::PI;
    let mercator_y = (f64::from(tile.y) + 0.5) / tile_count;
    let tangent_half_lat = (std::f64::consts::PI - mercator_y * std::f64::consts::TAU).exp();
    let tangent_half_lat_sq = tangent_half_lat * tangent_half_lat;
    let denom = tangent_half_lat_sq + 1.0;
    let sin_lat = (tangent_half_lat_sq - 1.0) / denom;
    let cos_lat = (2.0 * tangent_half_lat) / denom;
    let (sin_lon, cos_lon) = lon_center.sin_cos();
    TileAxes {
        center: Vector3::new(sin_lon * cos_lat, sin_lat, cos_lon * cos_lat),
        north: Vector3::new(-sin_lon * sin_lat, cos_lat, -cos_lon * sin_lat),
        east: Vector3::new(-lon_east.cos(), 0.0, lon_east.sin()),
        west: Vector3::new(lon_west.cos(), 0.0, -lon_west.sin()),
    }
}

fn extend_curved_edge_extremes(
    extremes: &mut Vec<Vector3<f64>>,
    tile: TileCoords,
    center: Vector3<f64>,
    max_radius: f64,
) {
    extremes.push(center * max_radius);
    let tile_count = 2_u32.pow(u32::from(u8::from(tile.z)));
    let edge_y = if tile.y >= tile_count / 2 {
        0.0
    } else {
        EXTENT
    };
    let edge_midpoint = project_tile_coordinates_to_unit_sphere(
        tile.x,
        tile.y,
        u8::from(tile.z),
        EXTENT / 2.0,
        edge_y,
    );
    extremes.push(edge_midpoint * max_radius);
}

fn tile_planes(points: &[Vector3<f64>], axes: TileAxes) -> [Vector4<f64>; 6] {
    let (up_min, up_max) = axis_min_max(axes.center, points);
    let (north_min, north_max) = axis_min_max(axes.north, points);
    [
        (-axes.center).extend(up_max),
        axes.center.extend(-up_min),
        (-axes.north).extend(north_max),
        axes.north.extend(-north_min),
        axes.east.extend(0.0),
        axes.west.extend(0.0),
    ]
}

fn axis_min_max(axis: Vector3<f64>, points: &[Vector3<f64>]) -> (f64, f64) {
    points
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(min, max), point| {
            let value = axis.dot(*point);
            (min.min(value), max.max(value))
        })
}

fn volume_points(
    tile: TileCoords,
    planes: [Vector4<f64>; 6],
) -> Result<Vec<Vector3<f64>>, GlobeTileBoundsError> {
    let [up, down, north, south, east, west] = planes;
    let tile_count = 2_u32.pow(u32::from(u8::from(tile.z)));
    let mut points = Vec::with_capacity(8);
    if tile.y == 0 {
        points.push(polar_axis_intersection(up).ok_or_else(|| bounds_error(tile))?);
        points.push(polar_axis_intersection(down).ok_or_else(|| bounds_error(tile))?);
    } else {
        for (first, second, third) in [
            (north, east, up),
            (north, east, down),
            (north, west, up),
            (north, west, down),
        ] {
            points.push(
                three_plane_intersection(first, second, third).ok_or_else(|| bounds_error(tile))?,
            );
        }
    }
    if tile.y == tile_count - 1 {
        points.push(polar_axis_intersection(up).ok_or_else(|| bounds_error(tile))?);
        points.push(polar_axis_intersection(down).ok_or_else(|| bounds_error(tile))?);
    } else {
        for (first, second, third) in [
            (south, east, up),
            (south, east, down),
            (south, west, up),
            (south, west, down),
        ] {
            points.push(
                three_plane_intersection(first, second, third).ok_or_else(|| bounds_error(tile))?,
            );
        }
    }
    Ok(points)
}

/// Intersects the globe's polar axis (`x = 0, z = 0`, the exact intersection line of any two
/// distinct meridian planes) with `plane` without high-zoom `west.cross(east)` cancellation.
fn polar_axis_intersection(plane: Vector4<f64>) -> Option<Vector3<f64>> {
    (plane.y.abs() > f64::EPSILON).then(|| Vector3::new(0.0, -plane.w / plane.y, 0.0))
}

fn three_plane_intersection(
    first: Vector4<f64>,
    second: Vector4<f64>,
    third: Vector4<f64>,
) -> Option<Vector3<f64>> {
    let first_normal = first.truncate();
    let second_normal = second.truncate();
    let third_normal = third.truncate();
    let determinant = first_normal.dot(second_normal.cross(third_normal));
    if determinant.abs() <= f64::EPSILON {
        return None;
    }
    Some(
        (second_normal.cross(third_normal) * -first.w
            + third_normal.cross(first_normal) * -second.w
            + first_normal.cross(second_normal) * -third.w)
            / determinant,
    )
}

fn bounds_error(tile: TileCoords) -> GlobeTileBoundsError {
    GlobeTileBoundsError::DegeneratePlanes {
        z: u8::from(tile.z),
        x: tile.x,
        y: tile.y,
    }
}

#[cfg(test)]
mod tests;

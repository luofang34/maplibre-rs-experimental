//! Closest-point distance queries on unit-sphere globe tiles.

use cgmath::{InnerSpace, Vector3, Vector4};

use super::{super::mercator_to_angular_radians, GlobeTileBoundsError};
use crate::coords::TileCoords;
pub(crate) use crate::projection::sphere::Intersection;

/// Closest point on `tile`'s spherical quadrangle (including polar extensions at `y == 0` and
/// `y == 2^z - 1`) to `eye_hat` on `S^2`, returning `(closest_point, chebyshev_chord)` where
/// `chebyshev_chord = max(chord_lon, chord_lat)` decomposes `||eye_hat - closest_point||` cleanly.
pub(crate) fn closest_unit_sphere_point_on_tile(
    eye_hat: Vector3<f64>,
    tile: TileCoords,
) -> (Vector3<f64>, f64) {
    closest_unit_sphere_point_in_lat_band(eye_hat, tile, true)
}

/// Closest point on `tile`'s actual Web Mercator data quadrangle (`|lat| <= 85.051129°`, excluding
/// the synthetic data-free polar fan) to `eye_hat` on `S^2`, returning
/// `(closest_point, chebyshev_chord)`.
pub(crate) fn closest_mercator_data_point_on_tile(
    eye_hat: Vector3<f64>,
    tile: TileCoords,
) -> (Vector3<f64>, f64) {
    closest_unit_sphere_point_in_lat_band(eye_hat, tile, false)
}

fn closest_unit_sphere_point_in_lat_band(
    eye_hat: Vector3<f64>,
    tile: TileCoords,
    include_poles: bool,
) -> (Vector3<f64>, f64) {
    let zoom = u8::from(tile.z);
    if zoom == 0 && include_poles {
        return (eye_hat, 0.0);
    }
    let tile_count = 1_u32 << u32::from(zoom);
    let scale = f64::from(tile_count);
    let (sin_north, cos_north) = if include_poles && tile.y == 0 {
        (1.0, 0.0)
    } else {
        mercator_to_angular_radians(0.5, f64::from(tile.y) / scale)
            .1
            .sin_cos()
    };
    let (sin_south, cos_south) = if include_poles && tile.y == tile_count - 1 {
        (-1.0, 0.0)
    } else {
        mercator_to_angular_radians(0.5, f64::from(tile.y + 1) / scale)
            .1
            .sin_cos()
    };
    let r_xz = eye_hat.x.hypot(eye_hat.z);
    let (hx, hz, a) = closest_meridian(eye_hat, tile.x, zoom, scale, r_xz);
    let m = a.hypot(eye_hat.y);
    let (sin_lat, cos_lat) =
        if a >= 0.0 && m > f64::EPSILON && eye_hat.y >= m * sin_south && eye_hat.y <= m * sin_north
        {
            (eye_hat.y / m, a / m)
        } else if a * cos_north + eye_hat.y * sin_north >= a * cos_south + eye_hat.y * sin_south {
            (sin_north, cos_north)
        } else {
            (sin_south, cos_south)
        };
    let q_hat = Vector3::new(hx * cos_lat, sin_lat, hz * cos_lat);
    let chord_lat = (r_xz - cos_lat).hypot(eye_hat.y - sin_lat);
    let chord_lon = (2.0 * cos_lat * (r_xz - a)).max(0.0).sqrt();
    (q_hat, chord_lon.max(chord_lat))
}

fn closest_meridian(
    eye_hat: Vector3<f64>,
    tile_x: u32,
    zoom: u8,
    scale: f64,
    r_xz: f64,
) -> (f64, f64, f64) {
    if zoom == 0 {
        return if r_xz <= f64::EPSILON {
            (0.0, 1.0, 0.0)
        } else {
            (eye_hat.x / r_xz, eye_hat.z / r_xz, r_xz)
        };
    }
    let lon_min = (f64::from(tile_x) / scale) * std::f64::consts::TAU + std::f64::consts::PI;
    let lon_max = (f64::from(tile_x + 1) / scale) * std::f64::consts::TAU + std::f64::consts::PI;
    let lon_mid = 0.5 * (lon_min + lon_max);
    let half_dlon = std::f64::consts::PI / scale;
    if r_xz <= f64::EPSILON {
        let (s, c) = lon_mid.sin_cos();
        (s, c, 0.0)
    } else {
        let (sin_mid, cos_mid) = lon_mid.sin_cos();
        if eye_hat.x * sin_mid + eye_hat.z * cos_mid >= r_xz * half_dlon.cos() {
            (eye_hat.x / r_xz, eye_hat.z / r_xz, r_xz)
        } else {
            let (sin_min, cos_min) = lon_min.sin_cos();
            let (sin_max, cos_max) = lon_max.sin_cos();
            let g_min = eye_hat.x * sin_min + eye_hat.z * cos_min;
            let g_max = eye_hat.x * sin_max + eye_hat.z * cos_max;
            if g_min >= g_max {
                (sin_min, cos_min, g_min)
            } else {
                (sin_max, cos_max, g_max)
            }
        }
    }
}

/// Intersects the globe's polar axis (`x = 0, z = 0`, the exact intersection line of any two
/// distinct meridian planes) with `plane` without high-zoom `west.cross(east)` cancellation.
pub(super) fn polar_axis_intersection(plane: Vector4<f64>) -> Option<Vector3<f64>> {
    (plane.y.abs() > f64::EPSILON).then(|| Vector3::new(0.0, -plane.w / plane.y, 0.0))
}

pub(super) fn three_plane_intersection(
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

pub(super) fn bounds_error(tile: TileCoords) -> GlobeTileBoundsError {
    GlobeTileBoundsError::DegeneratePlanes {
        z: u8::from(tile.z),
        x: tile.x,
        y: tile.y,
    }
}

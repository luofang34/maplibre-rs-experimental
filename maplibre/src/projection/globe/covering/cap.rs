//! Singularity-free unit-sphere spherical cap bounds for canonical globe tiles.

use std::cell::RefCell;

use cgmath::{InnerSpace, Vector3, Vector4};

use super::super::{
    angular_radians_to_unit_sphere, covering_tiles::Intersection, mercator_to_angular_radians,
    project_tile_coordinates_to_unit_sphere,
};
use crate::coords::{TileCoords, ZoomLevel, EXTENT};

const CAP_CACHE_LEN: usize = 2048;

thread_local! {
    static CAP_CACHE: RefCell<Vec<(TileCoords, TileSphericalCap)>> = const { RefCell::new(Vec::new()) };
}

#[inline(always)]
fn cap_cache_slot(tile: TileCoords) -> usize {
    let mut h = u64::from(tile.x).wrapping_mul(0x517c_c1b7_2722_0a95)
        ^ u64::from(tile.y).wrapping_mul(0x9e37_79b9_7f4a_7c15)
        ^ u64::from(u8::from(tile.z)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    h ^= h >> 32;
    (h as usize) & (CAP_CACHE_LEN - 1)
}

/// Singularity-free unit-sphere spherical cap `{ x in S^2 : x . center >= cos_alpha }`
/// enclosing a canonical globe tile (including its polar fan at `y == 0` or `y == 2^z - 1`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TileSphericalCap {
    /// Outward unit-sphere axis of the cap (`||center|| = 1`).
    pub center: Vector3<f64>,
    /// Cosine of the angular half-aperture `alpha` (`-1.0` for `z == 0`, hemisphere `~0.0` for
    /// `z == 1`, and strictly positive for `z >= 2`).
    pub cos_alpha: f64,
    /// Sine of the angular half-aperture `alpha` (`0.0` for `z == 0`, `in [0.0, 1.0]` for `z >= 1`).
    pub sin_alpha: f64,
}

impl TileSphericalCap {
    /// Classifies the radial cap shell `[min_radius, max_radius] * Cap` against the half-space
    /// `plane.truncate().dot(p) + plane.w >= 0.0`.
    pub(crate) fn shell_intersects_plane(
        &self,
        plane: Vector4<f64>,
        min_radius: f64,
        max_radius: f64,
    ) -> Intersection {
        let normal = plane.truncate();
        let len = normal.magnitude();
        if len <= f64::EPSILON {
            return if plane.w >= 0.0 {
                Intersection::Full
            } else {
                Intersection::None
            };
        }
        let n = normal / len;
        let d = plane.w / len;
        let (unit_max, unit_min) = if self.cos_alpha <= -1.0 {
            (1.0, -1.0)
        } else {
            let u = n.dot(self.center).clamp(-1.0, 1.0);
            let s = (1.0 - u * u).max(0.0).sqrt();
            let hi = if u >= self.cos_alpha {
                1.0
            } else {
                u * self.cos_alpha + s * self.sin_alpha
            };
            let lo = if -u >= self.cos_alpha {
                -1.0
            } else {
                u * self.cos_alpha - s * self.sin_alpha
            };
            (hi, lo)
        };
        let max_val = if unit_max >= 0.0 {
            max_radius * unit_max
        } else {
            min_radius * unit_max
        } + d;
        if max_val < 0.0 {
            return Intersection::None;
        }
        let min_val = if unit_min >= 0.0 {
            min_radius * unit_min
        } else {
            max_radius * unit_min
        } + d;
        if min_val >= 0.0 {
            Intersection::Full
        } else {
            Intersection::Partial
        }
    }
}

/// Computes the tight unit-sphere spherical cap enclosing `tile` (including polar fans).
pub fn tile_spherical_cap(tile: TileCoords) -> TileSphericalCap {
    let zoom = u8::from(tile.z);
    if zoom == 0 {
        return TileSphericalCap {
            center: Vector3::unit_z(),
            cos_alpha: -1.0,
            sin_alpha: 0.0,
        };
    }
    if zoom < 32 {
        let slot = cap_cache_slot(tile);
        if let Some(hit) = CAP_CACHE.with(|cache| {
            let borrow = cache.borrow();
            borrow
                .get(slot)
                .filter(|(cached, _)| *cached == tile)
                .map(|(_, cap)| *cap)
        }) {
            return hit;
        }
        let computed = compute_tile_spherical_cap(tile, zoom);
        CAP_CACHE.with(|cache| {
            let mut borrow = cache.borrow_mut();
            if borrow.is_empty() {
                let sentinel = (
                    TileCoords {
                        x: u32::MAX,
                        y: u32::MAX,
                        z: ZoomLevel::new(u8::MAX),
                    },
                    TileSphericalCap {
                        center: Vector3::unit_z(),
                        cos_alpha: -1.0,
                        sin_alpha: 0.0,
                    },
                );
                borrow.resize(CAP_CACHE_LEN, sentinel);
            }
            borrow[slot] = (tile, computed);
        });
        return computed;
    }
    compute_tile_spherical_cap(tile, zoom)
}

fn compute_tile_spherical_cap(tile: TileCoords, zoom: u8) -> TileSphericalCap {
    let tile_count = 1_u32 << u32::from(zoom);
    let scale = f64::from(tile_count);
    let (lon_mid, _) = mercator_to_angular_radians((f64::from(tile.x) + 0.5) / scale, 0.5);
    let lat_north = if tile.y == 0 {
        std::f64::consts::FRAC_PI_2
    } else {
        mercator_to_angular_radians(0.5, f64::from(tile.y) / scale).1
    };
    let lat_south = if tile.y == tile_count - 1 {
        -std::f64::consts::FRAC_PI_2
    } else {
        mercator_to_angular_radians(0.5, f64::from(tile.y + 1) / scale).1
    };
    let center = angular_radians_to_unit_sphere(lon_mid, 0.5 * (lat_north + lat_south));
    let corners = super::tile_corners(tile);
    let north_mid =
        project_tile_coordinates_to_unit_sphere(tile.x, tile.y, zoom, EXTENT * 0.5, 0.0);
    let south_mid =
        project_tile_coordinates_to_unit_sphere(tile.x, tile.y, zoom, EXTENT * 0.5, EXTENT);
    let mut cos_alpha = corners
        .into_iter()
        .chain([north_mid, south_mid])
        .map(|point| center.dot(point))
        .fold(1.0_f64, f64::min);
    if tile.y == 0 {
        cos_alpha = cos_alpha.min(center.dot(Vector3::unit_y()));
    }
    if tile.y == tile_count - 1 {
        cos_alpha = cos_alpha.min(center.dot(-Vector3::unit_y()));
    }
    let cos_alpha = (cos_alpha - 1e-12).clamp(-1.0, 1.0);
    let sin_alpha = (1.0 - cos_alpha * cos_alpha).max(0.0).sqrt();
    TileSphericalCap {
        center,
        cos_alpha,
        sin_alpha,
    }
}

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

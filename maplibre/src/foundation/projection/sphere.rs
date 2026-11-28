//! Shared unit-sphere projections and canonical tile spherical caps.

use std::{cell::RefCell, f64::consts::PI};

use cgmath::{InnerSpace, Vector3, Vector4};

use crate::coords::{LatLon, TileCoords, ZoomLevel, EXTENT};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Intersection {
    None,
    Partial,
    Full,
}

const CAP_CACHE_LEN: usize = if cfg!(target_arch = "wasm32") {
    1024
} else {
    2048
};

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

/// Converts normalized Web Mercator coordinates to longitude and latitude in radians.
pub fn mercator_to_angular_radians(mercator_x: f64, mercator_y: f64) -> (f64, f64) {
    let longitude = (mercator_x * PI * 2.0 + PI).rem_euclid(PI * 2.0);
    let latitude = 2.0 * (PI - mercator_y * PI * 2.0).exp().atan() - PI * 0.5;
    (longitude, latitude)
}

/// Converts longitude and latitude in radians to a point on the unit sphere.
pub fn angular_radians_to_unit_sphere(longitude: f64, latitude: f64) -> Vector3<f64> {
    let horizontal_length = latitude.cos();
    Vector3::new(
        longitude.sin() * horizontal_length,
        latitude.sin(),
        longitude.cos() * horizontal_length,
    )
}

/// Projects tile-local Web Mercator coordinates onto a unit sphere.
///
/// Tile coordinates use the crate's [`EXTENT`] and XYZ addressing. Positive Y points north and
/// longitude and latitude zero map to positive Z.
pub fn project_tile_coordinates_to_unit_sphere(
    tile_x: u32,
    tile_y: u32,
    zoom: u8,
    in_tile_x: f64,
    in_tile_y: f64,
) -> Vector3<f64> {
    let tile_count = crate::coords::exp2_u8(zoom);
    let mercator_x = (f64::from(tile_x) + in_tile_x / EXTENT) / tile_count;
    let mercator_y = (f64::from(tile_y) + in_tile_y / EXTENT) / tile_count;
    let longitude = mercator_x * PI * 2.0 + PI;

    // This form preserves equatorial precision on GPUs where subtracting PI / 2 after inverse
    // Web Mercator conversion discards significant float32 mantissa bits.
    let exp_arg = (PI - mercator_y * PI * 2.0).clamp(-300.0, 300.0);
    let tangent_half_latitude = exp_arg.exp();
    let tangent_half_latitude_squared = tangent_half_latitude * tangent_half_latitude;
    let denominator = tangent_half_latitude_squared + 1.0;
    let sin_latitude = (tangent_half_latitude_squared - 1.0) / denominator;
    let cos_latitude = (2.0 * tangent_half_latitude) / denominator;
    let (sin_lon, cos_lon) = longitude.sin_cos();

    Vector3::new(sin_lon * cos_latitude, sin_latitude, cos_lon * cos_latitude)
}

/// Converts a geographic location in degrees to a point on the unit sphere.
pub fn lat_lon_to_unit_sphere(location: LatLon) -> Vector3<f64> {
    angular_radians_to_unit_sphere(
        location.longitude.to_radians(),
        location.latitude.to_radians(),
    )
}

pub(crate) fn tile_corners(tile: TileCoords) -> [Vector3<f64>; 4] {
    let zoom = u8::from(tile.z);
    [
        project_tile_coordinates_to_unit_sphere(tile.x, tile.y, zoom, 0.0, 0.0),
        project_tile_coordinates_to_unit_sphere(tile.x, tile.y, zoom, EXTENT, 0.0),
        project_tile_coordinates_to_unit_sphere(tile.x, tile.y, zoom, EXTENT, EXTENT),
        project_tile_coordinates_to_unit_sphere(tile.x, tile.y, zoom, 0.0, EXTENT),
    ]
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
    #[cfg(test)]
    pub(crate) fn shell_intersects_plane(
        &self,
        plane: Vector4<f64>,
        min_radius: f64,
        max_radius: f64,
    ) -> Intersection {
        self.shell_intersects_plane_with_margin(plane, min_radius, max_radius)
            .0
    }

    /// Classifies the radial cap shell and returns `(intersection, max_val, min_val)` in
    /// normalized plane distance units.
    pub(crate) fn shell_intersects_plane_with_margin(
        &self,
        plane: Vector4<f64>,
        min_radius: f64,
        max_radius: f64,
    ) -> (Intersection, f64, f64) {
        let normal = plane.truncate();
        let len = normal.magnitude();
        if len <= f64::EPSILON {
            return if plane.w >= 0.0 {
                (Intersection::Full, 0.0, f64::INFINITY)
            } else {
                (Intersection::None, f64::NEG_INFINITY, 0.0)
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
            return (Intersection::None, max_val, f64::NEG_INFINITY);
        }
        let min_val = if unit_min >= 0.0 {
            min_radius * unit_min
        } else {
            max_radius * unit_min
        } + d;
        if min_val >= 0.0 {
            (Intersection::Full, max_val, min_val)
        } else {
            (Intersection::Partial, max_val, min_val)
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
            if let Some(entry) = borrow.get_mut(slot) {
                *entry = (tile, computed);
            }
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
    let corners = tile_corners(tile);
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

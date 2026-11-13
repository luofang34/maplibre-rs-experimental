//! Coordinate spaces, map zoom and visible tile regions.

#![deny(missing_docs)]

pub(crate) mod clip;
mod overscaled;
mod tile;
mod view_region;
use std::{
    f64::consts::PI,
    fmt,
    fmt::{Display, Formatter},
};

use bytemuck_derive::{Pod, Zeroable};
#[cfg(test)]
use cgmath::Vector4;
use cgmath::{AbsDiffEq, Matrix4, Point2, Point3, Vector3};
pub use overscaled::{OverscaledChildren, OverscaledTileId};
use serde::{Deserialize, Serialize};
pub use tile::{AlignedWorldTileCoords, InnerCoords, TileCoords, WorldTileCoords};
pub use view_region::ViewRegion;

use crate::{
    style::source::TileAddressingScheme,
    util::{math::div_floor, SignificantlyDifferent},
};

/// Unsigned tile-local extent used when normalizing decoded geometry.
pub const EXTENT_UINT: u32 = 4096;
/// Signed tile-local extent for geometry that can extend beyond a tile boundary.
pub const EXTENT_SINT: i32 = EXTENT_UINT as i32;
/// Tile-local extent at the precision used by coordinate transforms.
pub const EXTENT: f64 = EXTENT_UINT as f64;
#[cfg(test)]
const TOP_LEFT_EXTENT: Vector4<f64> = Vector4::new(0.0, 0.0, 0.0, 1.0);

/// Width of the entire Mercator world in pixels at zoom zero.
pub const TILE_SIZE: f64 = 512.0;
/// Number of discrete zoom levels represented by [`ZOOM_BOUNDS`], not an inclusive zoom limit.
pub const MAX_ZOOM: usize = 32;

/// Number of tile columns or rows at each supported grid level: `2^z` for `z` in `0..MAX_ZOOM`.
pub const ZOOM_BOUNDS: [u32; MAX_ZOOM] = create_zoom_bounds::<MAX_ZOOM>();

const fn create_zoom_bounds<const DIM: usize>() -> [u32; DIM] {
    let mut result: [u32; DIM] = [0; DIM];
    let mut i = 0;
    while i < DIM {
        result[i] = 2u32.pow(i as u32);
        i += 1;
    }
    result
}

/// Ordered tile key containing its zoom level and the quadrant digit for each coordinate bit pair.
#[derive(Ord, PartialOrd, Eq, PartialEq, Hash, Clone, Copy)]
pub struct Quadkey(pub(crate) u128);

impl Quadkey {
    /// Builds a key from quadrant digits, starting with the least-significant coordinate bits.
    /// The slice length determines the tile zoom; digit values are stored without validation.
    ///
    /// # Panics
    /// Panics when the slice contains [`MAX_ZOOM`] or more digits.
    pub fn new(quad_encoded: &[ZoomLevel]) -> Self {
        assert!(
            quad_encoded.len() < MAX_ZOOM,
            "quadkey length {} exceeds MAX_ZOOM - 1",
            quad_encoded.len()
        );
        let mut bits = u128::from(quad_encoded.len() as u8) << 120;
        for (i, part) in quad_encoded.iter().enumerate() {
            bits |= u128::from(part.0 & 0b11) << (118 - 2 * i);
        }
        Self(bits)
    }
}

impl fmt::Debug for Quadkey {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let level = (self.0 >> 120) as u8;
        if level > 0 {
            write!(f, "{:?}", ZoomLevel(level))?;
            for i in 0..(usize::from(level) - 1) {
                let digit = ((self.0 >> (118 - 2 * i)) & 0b11) as u8;
                write!(f, "{:?}", ZoomLevel(digit))?;
            }
        }
        Ok(())
    }
}

/// Exact `2.0_f64.powi(i32::from(z))` via IEEE-754 binary64 exponent construction (`z` in `0..=255`).
#[inline(always)]
pub(crate) const fn exp2_u8(z: u8) -> f64 {
    f64::from_bits((1023_u64 + (z as u64)) << 52)
}

/// Integer tile-grid level. Construction does not validate the bounds of a particular grid.
#[derive(
    Ord,
    PartialOrd,
    Eq,
    PartialEq,
    Hash,
    Copy,
    Clone,
    Debug,
    Default,
    Serialize,
    Deserialize,
    Pod,
    Zeroable,
)]
#[repr(C)]
pub struct ZoomLevel(u8);

impl ZoomLevel {
    /// Stores a level without range checking; grid-table lookups require `z < MAX_ZOOM`.
    pub const fn new(z: u8) -> Self {
        ZoomLevel(z)
    }
    /// Whether this level consists of the single zoom-zero world tile.
    pub fn is_root(self) -> bool {
        self.0 == 0
    }
    /// Exact grid dimension `2.0^z` as `f64` without calling `libm::pow`.
    #[inline(always)]
    pub const fn grid_scale(self) -> f64 {
        exp2_u8(self.0)
    }
}

impl std::ops::Add<u8> for ZoomLevel {
    type Output = ZoomLevel;

    #[allow(clippy::expect_used)]
    fn add(self, rhs: u8) -> Self::Output {
        let zoom_level = self.0.checked_add(rhs).expect("zoom level overflowed");
        ZoomLevel(zoom_level)
    }
}

impl std::ops::Sub<u8> for ZoomLevel {
    type Output = ZoomLevel;

    #[allow(clippy::expect_used)]
    fn sub(self, rhs: u8) -> Self::Output {
        let zoom_level = self.0.checked_sub(rhs).expect("zoom level underflowed");
        ZoomLevel(zoom_level)
    }
}

impl Display for ZoomLevel {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<u8> for ZoomLevel {
    fn from(zoom_level: u8) -> Self {
        ZoomLevel(zoom_level)
    }
}

impl From<ZoomLevel> for u8 {
    fn from(val: ZoomLevel) -> Self {
        val.0
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
/// Geographic coordinates in degrees, ordered latitude before longitude.
pub struct LatLon {
    /// Latitude in degrees north of the equator; no range is enforced by this type.
    pub latitude: f64,
    /// Longitude in degrees east of the prime meridian; no wrapping is performed by this type.
    pub longitude: f64,
}

impl LatLon {
    /// Stores coordinates in degrees without clamping latitude or wrapping longitude.
    pub fn new(latitude: f64, longitude: f64) -> Self {
        LatLon {
            latitude,
            longitude,
        }
    }

    /// Projects this geographic location to normalized Mercator coordinates in `[0..1] x [0..1]`.
    pub fn to_mercator(&self) -> Point2<f64> {
        let x = self.longitude / 360.0 + 0.5;
        let y = (1.0 - self.latitude.to_radians().tan().asinh() / PI) * 0.5;
        Point2::new(x, y.clamp(0.0, 1.0 - 1e-9))
    }

    /// Converts normalized Mercator coordinates in `[0..1] x [0..1]` back to geographic coordinates.
    pub fn from_mercator(mercator: Point2<f64>) -> Self {
        let lat = (PI * (1.0 - 2.0 * mercator.y)).sinh().atan().to_degrees();
        let lon = mercator.x * 360.0 - 180.0;
        LatLon::new(lat, lon)
    }
}

impl Default for LatLon {
    fn default() -> Self {
        LatLon {
            latitude: 0.0,
            longitude: 0.0,
        }
    }
}

impl Display for LatLon {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{},{}", self.latitude, self.longitude)
    }
}

/// Continuous map zoom: increasing it by one doubles the world-pixel scale.
#[derive(Copy, Clone, Debug)]
pub struct Zoom(f64);

impl Zoom {
    /// Stores a continuous zoom without validating finiteness or a source's supported range.
    pub fn new(zoom: f64) -> Self {
        Zoom(zoom)
    }

    /// Returns the continuous zoom value.
    pub fn value(self) -> f64 {
        self.0
    }

    /// Returns the continuous zoom at GPU precision; it does not round to a tile-grid level.
    pub fn level(&self) -> f32 {
        self.0 as f32
    }
}

impl Zoom {
    /// Converts a discrete tile level to a continuous zoom with no fractional component.
    pub fn from(zoom_level: ZoomLevel) -> Self {
        Zoom(zoom_level.0 as f64)
    }
}

impl Default for Zoom {
    fn default() -> Self {
        Zoom(0.0)
    }
}

impl Display for Zoom {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", (self.0 * 100.0).round() / 100.0)
    }
}

impl std::ops::Add for Zoom {
    type Output = Zoom;

    fn add(self, rhs: Self) -> Self::Output {
        Zoom(self.0 + rhs.0)
    }
}

impl std::ops::Sub for Zoom {
    type Output = Zoom;

    fn sub(self, rhs: Self) -> Self::Output {
        Zoom(self.0 - rhs.0)
    }
}

impl Zoom {
    /// Scale factor `2^(tile.z - self)` from this zoom to the tile's grid level.
    pub fn scale_to_tile(&self, coords: &WorldTileCoords) -> f64 {
        2.0_f64.powf(coords.z.0 as f64 - self.0)
    }

    /// Scale factor `2^(z - self)` from this zoom to a discrete grid level.
    pub fn scale_to_zoom_level(&self, z: ZoomLevel) -> f64 {
        2.0_f64.powf(z.0 as f64 - self.0)
    }

    /// Scale factor `2^(zoom - self)` for positions when changing to the supplied zoom.
    pub fn scale_delta(&self, zoom: &Zoom) -> f64 {
        2.0_f64.powf(zoom.0 - self.0)
    }

    /// Selects a tile level using `floor(zoom + log2(512 / tile_size))`.
    ///
    /// `tile_size` is the positive source tile width in pixels. A 256-pixel source selects one
    /// level higher than a 512-pixel source. Clamped to `0..MAX_ZOOM`, not source min/max zoom.
    pub fn zoom_level(&self, tile_size: f64) -> ZoomLevel {
        // TODO: Also support round() instead of floor() here
        let z = (self.0 + (TILE_SIZE / tile_size).log2())
            .floor()
            .clamp(0.0, (MAX_ZOOM - 1) as f64) as u8;
        ZoomLevel(z)
    }
}

impl SignificantlyDifferent for Zoom {
    type Epsilon = f64;

    fn ne(&self, other: &Self, epsilon: Self::Epsilon) -> bool {
        self.0.abs_diff_ne(&other.0, epsilon)
    }
}

/// Mercator coordinates measured in world pixels at a particular zoom.
///
/// # Coordinate System Origin
///
/// The origin of the coordinate system is in the upper-left corner.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct WorldCoords {
    /// Pixel distance east from the Mercator world's western edge at the associated zoom.
    pub x: f64,
    /// Pixel distance south from the Mercator world's northern edge at the associated zoom.
    pub y: f64,
}

impl WorldCoords {
    /// Projects degrees to Mercator pixels at `zoom` without clamping latitude or wrapping longitude.
    /// Callers must supply finite longitude and latitude strictly between -90 and 90 degrees.
    pub fn from_lat_lon(lat_lon: LatLon, zoom: Zoom) -> WorldCoords {
        let tile_size = TILE_SIZE * 2.0_f64.powf(zoom.0);
        // Get x value
        let x = (lat_lon.longitude + 180.0) * (tile_size / 360.0);

        // Convert from degrees to radians
        let lat_rad = (lat_lon.latitude * PI) / 180.0;

        // get y value
        let merc_n = f64::ln(f64::tan((PI / 4.0) + (lat_rad / 2.0)));
        let y = (tile_size / 2.0) - (tile_size * merc_n / (2.0 * PI));

        WorldCoords { x, y }
    }

    /// Stores a ground-plane pixel position; the associated zoom remains the caller's responsibility.
    pub fn at_ground(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// Converts pixels at `zoom` to tile indices at `z`, flooring fractional indices, so a point
    /// left of the world lies in tile `-1`. The returned indices are not checked against
    /// canonical tile bounds.
    pub fn into_world_tile(self, z: ZoomLevel, zoom: Zoom) -> WorldTileCoords {
        let tile_scale = zoom.scale_to_zoom_level(z) / TILE_SIZE; // TODO: Deduplicate
        let x = self.x * tile_scale;
        let y = self.y * tile_scale;

        WorldTileCoords {
            x: x.floor() as i32,
            y: y.floor() as i32,
            z,
        }
    }
}

impl From<(f32, f32)> for WorldCoords {
    fn from(tuple: (f32, f32)) -> Self {
        WorldCoords {
            x: tuple.0 as f64,
            y: tuple.1 as f64,
        }
    }
}

impl From<(f64, f64)> for WorldCoords {
    fn from(tuple: (f64, f64)) -> Self {
        WorldCoords {
            x: tuple.0,
            y: tuple.1,
        }
    }
}

impl From<Point3<f64>> for WorldCoords {
    fn from(point: Point3<f64>) -> Self {
        WorldCoords {
            x: point.x,
            y: point.y,
        }
    }
}

impl Display for WorldCoords {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "W(x={x},y={y})", x = self.x, y = self.y,)
    }
}

#[cfg(test)]
mod tests;

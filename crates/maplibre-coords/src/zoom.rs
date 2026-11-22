//! Discrete zoom levels, continuous zoom scaling, and quadtree tile keys.

use std::{
    fmt,
    fmt::{Display, Formatter},
};

use bytemuck_derive::{Pod, Zeroable};
use cgmath::AbsDiffEq;
use serde::{Deserialize, Serialize};

use crate::{SignificantlyDifferent, WorldTileCoords, MAX_ZOOM, TILE_SIZE};

/// Number of tile columns or rows at each supported grid level: `2^z` for `z` in `0..MAX_ZOOM`.
pub const ZOOM_BOUNDS: [u32; MAX_ZOOM] = [
    1 << 0,
    1 << 1,
    1 << 2,
    1 << 3,
    1 << 4,
    1 << 5,
    1 << 6,
    1 << 7,
    1 << 8,
    1 << 9,
    1 << 10,
    1 << 11,
    1 << 12,
    1 << 13,
    1 << 14,
    1 << 15,
    1 << 16,
    1 << 17,
    1 << 18,
    1 << 19,
    1 << 20,
    1 << 21,
    1 << 22,
    1 << 23,
    1 << 24,
    1 << 25,
    1 << 26,
    1 << 27,
    1 << 28,
    1 << 29,
    1 << 30,
    1 << 31,
];

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
pub const fn exp2_u8(z: u8) -> f64 {
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
pub struct ZoomLevel(pub(crate) u8);

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

    fn add(self, rhs: u8) -> Self::Output {
        assert!(self.0.checked_add(rhs).is_some(), "zoom level overflowed");
        ZoomLevel(self.0.wrapping_add(rhs))
    }
}

impl std::ops::Sub<u8> for ZoomLevel {
    type Output = ZoomLevel;

    fn sub(self, rhs: u8) -> Self::Output {
        assert!(self.0.checked_sub(rhs).is_some(), "zoom level underflowed");
        ZoomLevel(self.0.wrapping_sub(rhs))
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

    /// Converts a discrete tile level to a continuous zoom with no fractional component.
    pub fn from(zoom_level: ZoomLevel) -> Self {
        Zoom(zoom_level.0 as f64)
    }

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
        let z = (self.0 + (TILE_SIZE / tile_size).log2())
            .floor()
            .clamp(0.0, (MAX_ZOOM - 1) as f64) as u8;
        ZoomLevel(z)
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

impl SignificantlyDifferent for Zoom {
    type Epsilon = f64;

    fn ne(&self, other: &Self, epsilon: Self::Epsilon) -> bool {
        self.0.abs_diff_ne(&other.0, epsilon)
    }
}

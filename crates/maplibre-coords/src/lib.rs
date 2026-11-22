//! Coordinate spaces, map zoom and visible tile regions.

#![cfg_attr(
    not(test),
    forbid(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::todo,
        clippy::unimplemented
    )
)]
#![forbid(unsafe_code)]
#![deny(dead_code, unused_imports, missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod bounds;
mod overscaled;
#[cfg(test)]
#[path = "proptests_tests.rs"]
mod proptests;
mod scheme;
#[cfg(test)]
mod tests;
mod tile;
mod view_region;
mod world;
mod zoom;

pub use bounds::{div_floor, Aabb2, Aabb3};
#[cfg(test)]
use cgmath::Vector4;
pub use overscaled::{OverscaledChildren, OverscaledTileId};
pub use scheme::{SignificantlyDifferent, TileAddressingScheme};
pub use tile::{AlignedWorldTileCoords, InnerCoords, TileCoords, WorldTileCoords};
pub use view_region::ViewRegion;
pub use world::{LatLon, WorldCoords};
pub use zoom::{exp2_u8, Quadkey, Zoom, ZoomLevel, ZOOM_BOUNDS};

/// Unsigned tile-local extent used when normalizing decoded geometry.
pub const EXTENT_UINT: u32 = 4096;
/// Signed tile-local extent for geometry that can extend beyond a tile boundary.
pub const EXTENT_SINT: i32 = EXTENT_UINT as i32;
/// Tile-local extent at the precision used by coordinate transforms.
pub const EXTENT: f64 = EXTENT_UINT as f64;
#[cfg(test)]
pub(crate) const TOP_LEFT_EXTENT: Vector4<f64> = Vector4::new(0.0, 0.0, 0.0, 1.0);

/// Width of the entire Mercator world in pixels at zoom zero.
pub const TILE_SIZE: f64 = 512.0;
/// Number of discrete zoom levels represented by [`ZOOM_BOUNDS`], not an inclusive zoom limit.
pub const MAX_ZOOM: usize = 32;

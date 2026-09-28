//! Coordinate spaces, map zoom and visible tile regions.
mod tile;
use std::{
    f64::consts::PI,
    fmt,
    fmt::{Display, Formatter},
};

use bytemuck_derive::{Pod, Zeroable};
#[cfg(test)]
use cgmath::Vector4;
use cgmath::{AbsDiffEq, Matrix4, Point3, Vector3};
use serde::{Deserialize, Serialize};
pub use tile::{AlignedWorldTileCoords, InnerCoords, TileCoords, WorldTileCoords};

use crate::{
    style::source::TileAddressingScheme,
    util::{
        math::{div_floor, Aabb2},
        SignificantlyDifferent,
    },
};

pub const EXTENT_UINT: u32 = 4096;
pub const EXTENT_SINT: i32 = EXTENT_UINT as i32;
pub const EXTENT: f64 = EXTENT_UINT as f64;
#[cfg(test)]
const TOP_LEFT_EXTENT: Vector4<f64> = Vector4::new(0.0, 0.0, 0.0, 1.0);

pub const TILE_SIZE: f64 = 512.0;
pub const MAX_ZOOM: usize = 32;

// FIXME: MAX_ZOOM is 32, which means max bound is 2^32, which wouldn't fit in u32 or i32
// Bounds are generated 0..=31
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

/// Represents the position of a node within a quad tree. The first u8 defines the `ZoomLevel` of the node.
/// The remaining bytes define which part (north west, south west, south east, north east) of each
/// subdivision of the quadtree is concerned.
///
/// TODO: We can optimize the quadkey and store the keys on 2 bits instead of 8
#[derive(Ord, PartialOrd, Eq, PartialEq, Clone, Copy)]
pub struct Quadkey([ZoomLevel; MAX_ZOOM]);

impl Quadkey {
    pub fn new(quad_encoded: &[ZoomLevel]) -> Self {
        let mut key = [ZoomLevel::default(); MAX_ZOOM];
        key[0] = (quad_encoded.len() as u8).into();
        for (i, part) in quad_encoded.iter().enumerate() {
            key[i + 1] = *part;
        }
        Self(key)
    }
}

impl fmt::Debug for Quadkey {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let key = self.0;
        let ZoomLevel(level) = key[0];
        let len = level as usize;
        for part in &self.0[0..len] {
            write!(f, "{part:?}")?;
        }
        Ok(())
    }
}

// FIXME: does Pod and Zeroable make sense?
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
    pub const fn new(z: u8) -> Self {
        ZoomLevel(z)
    }
    pub fn is_root(self) -> bool {
        self.0 == 0
    }
}

impl std::ops::Add<u8> for ZoomLevel {
    type Output = ZoomLevel;

    fn add(self, rhs: u8) -> Self::Output {
        let zoom_level = self.0.checked_add(rhs).expect("zoom level overflowed");
        ZoomLevel(zoom_level)
    }
}

impl std::ops::Sub<u8> for ZoomLevel {
    type Output = ZoomLevel;

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
pub struct LatLon {
    pub latitude: f64,
    pub longitude: f64,
}

impl LatLon {
    pub fn new(latitude: f64, longitude: f64) -> Self {
        LatLon {
            latitude,
            longitude,
        }
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

/// `Zoom` is an exponential scale that defines the zoom of the camera on the map.
/// We can derive the `ZoomLevel` from `Zoom` by using the `[crate::coords::ZOOM_BOUNDS]`.
#[derive(Copy, Clone, Debug)]
pub struct Zoom(f64);

impl Zoom {
    pub fn new(zoom: f64) -> Self {
        Zoom(zoom)
    }

    /// Returns the continuous zoom value.
    pub fn value(self) -> f64 {
        self.0
    }

    pub fn level(&self) -> f32 {
        self.0 as f32
    }
}

impl Zoom {
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
    pub fn scale_to_tile(&self, coords: &WorldTileCoords) -> f64 {
        2.0_f64.powf(coords.z.0 as f64 - self.0)
    }

    pub fn scale_to_zoom_level(&self, z: ZoomLevel) -> f64 {
        2.0_f64.powf(z.0 as f64 - self.0)
    }

    pub fn scale_delta(&self, zoom: &Zoom) -> f64 {
        2.0_f64.powf(zoom.0 - self.0)
    }

    /// Adopted from
    /// [Transform::coveringZoomLevel](https://github.com/maplibre/maplibre-gl-js/blob/80e232a64716779bfff841dbc18fddc1f51535ad/src/geo/transform.ts#L279-L288)
    ///
    /// This function calculates which ZoomLevel to show at this zoom.
    ///
    /// The `tile_size` is the size of the tile like specified in the source definition,
    /// For example raster tiles can be 512px or 256px. If it is 256px, then 2x as many tiles are
    /// displayed. If the raster tile is 512px then exactly as many raster tiles like vector
    /// tiles would be displayed.
    pub fn zoom_level(&self, tile_size: f64) -> ZoomLevel {
        // TODO: Also support round() instead of floor() here
        let z = (self.0 + (TILE_SIZE / tile_size).ln() / 2.0_f64.ln()).floor() as u8;
        ZoomLevel(z)
    }
}

impl SignificantlyDifferent for Zoom {
    type Epsilon = f64;

    fn ne(&self, other: &Self, epsilon: Self::Epsilon) -> bool {
        self.0.abs_diff_eq(&other.0, epsilon)
    }
}

/// Mercator coordinates measured in world pixels at a particular zoom.
///
/// # Coordinate System Origin
///
/// The origin of the coordinate system is in the upper-left corner.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct WorldCoords {
    pub x: f64,
    pub y: f64,
}

impl WorldCoords {
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

    pub fn at_ground(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    pub fn into_world_tile(self, z: ZoomLevel, zoom: Zoom) -> WorldTileCoords {
        let tile_scale = zoom.scale_to_zoom_level(z) / TILE_SIZE; // TODO: Deduplicate
        let x = self.x * tile_scale;
        let y = self.y * tile_scale;

        WorldTileCoords {
            x: x as i32,
            y: y as i32,
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

/// Defines a bounding box on a tiled map with a [`ZoomLevel`] and a padding.
#[derive(Debug)]
pub struct ViewRegion {
    min_tile: WorldTileCoords,
    max_tile: WorldTileCoords,
    explicit_tiles: Option<Vec<WorldTileCoords>>,
    /// At which zoom level does this region exist
    zoom_level: ZoomLevel,
    /// Padding around this view region
    padding: i32,
    /// The maximum amount of tiles this view region contains
    max_n_tiles: usize,
}

impl ViewRegion {
    pub fn new(
        view_region: Aabb2<f64>,
        padding: i32,
        max_n_tiles: usize,
        zoom: Zoom,
        z: ZoomLevel,
    ) -> Self {
        let min_world: WorldCoords = WorldCoords::at_ground(view_region.min.x, view_region.min.y);
        let min_world_tile: WorldTileCoords = min_world.into_world_tile(z, zoom);
        let max_world: WorldCoords = WorldCoords::at_ground(view_region.max.x, view_region.max.y);
        let max_world_tile: WorldTileCoords = max_world.into_world_tile(z, zoom);

        Self {
            min_tile: min_world_tile,
            max_tile: max_world_tile,
            explicit_tiles: None,
            zoom_level: z,
            max_n_tiles,
            padding,
        }
    }

    /// Creates a region from an explicit projection-aware tile selection.
    pub fn from_tiles(
        tiles: Vec<WorldTileCoords>,
        zoom_level: ZoomLevel,
        max_n_tiles: usize,
    ) -> Self {
        Self {
            min_tile: WorldTileCoords::default(),
            max_tile: WorldTileCoords::default(),
            explicit_tiles: Some(tiles),
            zoom_level,
            max_n_tiles,
            padding: 0,
        }
    }

    pub fn zoom_level(&self) -> ZoomLevel {
        self.zoom_level
    }

    pub fn is_in_view(&self, &world_coords: &WorldTileCoords) -> bool {
        if let Some(tiles) = &self.explicit_tiles {
            return tiles.contains(&world_coords);
        }
        world_coords.x <= self.max_tile.x + self.padding
            && world_coords.y <= self.max_tile.y + self.padding
            && world_coords.x >= self.min_tile.x - self.padding
            && world_coords.y >= self.min_tile.y - self.padding
            && world_coords.z == self.zoom_level
    }

    pub fn iter(&self) -> Box<dyn Iterator<Item = WorldTileCoords> + '_> {
        if let Some(tiles) = &self.explicit_tiles {
            return Box::new(tiles.iter().copied().take(self.max_n_tiles));
        }
        Box::new(
            (self.min_tile.x - self.padding..self.max_tile.x + 1 + self.padding)
                .flat_map(move |x| {
                    (self.min_tile.y - self.padding..self.max_tile.y + 1 + self.padding)
                        .map(move |y| (x, y, self.zoom_level).into())
                })
                .take(self.max_n_tiles),
        )
    }
}

impl Display for WorldCoords {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "W(x={x},y={y})", x = self.x, y = self.y,)
    }
}

#[cfg(test)]
mod tests;

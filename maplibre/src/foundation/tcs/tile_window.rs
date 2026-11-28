//! Tile-local geographic clipping windows in normalized `[0, 1]` world coordinates.

use crate::coords::{WorldTileCoords, EXTENT};

/// Distance beyond the tile edge whose features are included, in tile grid units (GL JS default).
const BUFFER: f64 = 128.0;

/// Where a tile lies in the world, and the area its features are taken from.
pub(crate) struct TileWindow {
    /// Tiles across the world at the tile's zoom.
    pub(crate) tiles: f64,
    /// World x of the tile's western edge, its column wrapped onto the world.
    pub(crate) west: f64,
    /// World y of the tile's northern edge.
    pub(crate) north: f64,
    /// `[west, north, east, south]` of the tile and buffer around it.
    pub(crate) bounds: [f64; 4],
}

impl TileWindow {
    /// Builds the world-coordinate clipping window of `coords`.
    pub(crate) fn of(coords: WorldTileCoords) -> Self {
        let span = tile_grid_span(u8::from(coords.z));
        let tiles = f64::from(span);
        let west = f64::from(coords.x.rem_euclid(span as i32)) / tiles;
        let north = f64::from(coords.y) / tiles;
        let margin = BUFFER / EXTENT / tiles;
        Self {
            tiles,
            west,
            north,
            bounds: tile_window_bounds(west, north, tiles, margin),
        }
    }

    /// Whether `bounds` meets the window, edges included, as index lookups count it.
    pub(crate) fn meets(&self, bounds: &[f64; 4]) -> bool {
        ranges_overlap(bounds[0], bounds[2], self.bounds[0], self.bounds[2])
            && ranges_overlap(bounds[1], bounds[3], self.bounds[1], self.bounds[3])
    }
}

fn tile_grid_span(zoom: u8) -> u32 {
    1_u32 << u32::from(zoom).min(30)
}

fn tile_window_bounds(west: f64, north: f64, tiles: f64, margin: f64) -> [f64; 4] {
    let step = 1.0 / tiles;
    [
        west - margin,
        north - margin,
        west + step + margin,
        north + step + margin,
    ]
}

fn ranges_overlap(a_min: f64, a_max: f64, b_min: f64, b_max: f64) -> bool {
    a_min <= b_max && a_max >= b_min
}

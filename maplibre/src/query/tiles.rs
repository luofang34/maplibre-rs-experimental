//! The tiles a query reads and the rectangle it covers in each.

use super::ground::Footprint;
use crate::{
    coords::{WorldTileCoords, ZoomLevel, EXTENT, TILE_SIZE},
    io::geometry_index::{IndexedGeometry, TileIndex},
    render::resolved_view::ResolvedFrameView,
};

/// Tiles a single query reads at one zoom before it uses a coarser one.
const MAX_TILES: i64 = 256;

/// One tile a query touches: its canonical coordinates and the query rectangle in its grid.
pub(super) struct QueryTile {
    pub(super) coords: WorldTileCoords,
    /// Which copy of the world the tile is seen in: zero for the one at the antimeridian's east.
    pub(super) wrap: i32,
    /// Query rectangle in tile units, as `[min x, min y, max x, max y]`.
    pub(super) local: [f64; 4],
    /// Tile units that make one screen pixel.
    pub(super) units_per_pixel: f64,
    pub(super) zoom_level: u8,
    /// World pixels of the tile's top-left corner, in the copy of the world it is seen in.
    pub(super) origin: [f64; 2],
    /// World pixels in one tile unit.
    pub(super) world_per_unit: f64,
    /// The ground the query covers, in tile units, for features on the ground plane.
    pub(super) footprint: Option<Footprint>,
}

impl QueryTile {
    /// The footprint of ground corners given in world pixels.
    pub(super) fn footprint_of(&self, corners: &[[f64; 2]]) -> Option<Footprint> {
        let local: Vec<[f64; 2]> = corners
            .iter()
            .map(|corner| {
                [
                    (corner[0] - self.origin[0]) / self.world_per_unit,
                    (corner[1] - self.origin[1]) / self.world_per_unit,
                ]
            })
            .collect();
        Footprint::of(&local)
    }
}

/// The tiles at grid level `z` that a region of world pixels covers, or comes within `margin`
/// screen pixels of.
pub(super) fn tiles_in(
    region: [f64; 4],
    margin: f64,
    resolved: &ResolvedFrameView,
    z: u8,
) -> Vec<QueryTile> {
    let scale = resolved.zoom.scale_to_zoom_level(ZoomLevel::new(z));
    let to_grid = |world: f64| world / TILE_SIZE * scale;
    let (x0, y0) = (to_grid(region[0]), to_grid(region[1]));
    let (x1, y1) = (to_grid(region[2]), to_grid(region[3]));
    let mid_my = (((region[1] + region[3]) * 0.5) / resolved.world_size).clamp(0.0, 1.0);
    let reach = to_grid(margin * resolved.globe_scale_at_mercator_y(mid_my));
    let (tx0, tx1) = ((x0 - reach).floor() as i64, (x1 + reach).floor() as i64);
    let (ty0, ty1) = ((y0 - reach).floor() as i64, (y1 + reach).floor() as i64);
    let tiles_wide = 1_i64 << z;
    if (tx1 - tx0 + 1) * (ty1 - ty0 + 1) > MAX_TILES {
        return Vec::new();
    }
    let base_units_per_pixel = EXTENT * scale / TILE_SIZE;
    let mut tiles = Vec::new();
    for ty in ty0.max(0)..=ty1.min(tiles_wide - 1) {
        let row_my = ((ty as f64 + 0.5) / tiles_wide as f64).clamp(0.0, 1.0);
        let units_per_pixel = base_units_per_pixel * resolved.globe_scale_at_mercator_y(row_my);
        for tx in tx0..=tx1 {
            let canonical = tx.rem_euclid(tiles_wide);
            tiles.push(QueryTile {
                coords: WorldTileCoords {
                    x: canonical as i32,
                    y: ty as i32,
                    z: ZoomLevel::new(z),
                },
                wrap: tx.div_euclid(tiles_wide) as i32,
                local: [
                    (x0 - tx as f64) * EXTENT,
                    (y0 - ty as f64) * EXTENT,
                    (x1 - tx as f64) * EXTENT,
                    (y1 - ty as f64) * EXTENT,
                ],
                units_per_pixel,
                zoom_level: z,
                origin: [tx as f64 * TILE_SIZE / scale, ty as f64 * TILE_SIZE / scale],
                world_per_unit: TILE_SIZE / scale / EXTENT,
                footprint: None,
            });
        }
    }
    tiles
}

/// The geometries whose bounds meet the query rectangle.
pub(super) fn candidates_in(index: &TileIndex, local: [f64; 4]) -> Vec<&IndexedGeometry<f64>> {
    let window = rstar::AABB::from_corners(
        geo_types::Point::new(local[0], local[1]),
        geo_types::Point::new(local[2], local[3]),
    );
    match index {
        TileIndex::Spatial { tree } => tree.locate_in_envelope_intersecting(&window).collect(),
        TileIndex::Linear { list } => list
            .iter()
            .filter(|geometry| {
                use rstar::Envelope;
                geometry.bounds.intersects(&window)
            })
            .collect(),
    }
}

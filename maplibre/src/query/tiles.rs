//! The tiles a query reads and the rectangle it covers in each.

use cgmath::Vector2;

use super::{ground::Footprint, ViewState};
use crate::{
    coords::{WorldTileCoords, Zoom, ZoomLevel, EXTENT, TILE_SIZE},
    io::geometry_index::{IndexedGeometry, TileIndex},
};

/// Tiles a single query reads at one zoom before it uses a coarser one.
const MAX_TILES: i64 = 256;

/// The screen box with the point under the camera added, as GL JS `getCameraQueryGeometry`:
/// the base of any extrusion that stands up into the query lies between the two.
pub(super) fn camera_query_bounds(view_state: &ViewState, [x0, y0, x1, y1]: [f64; 4]) -> [f64; 4] {
    let offset = view_state.camera().get_pitch().0.tan() * view_state.camera_to_center_distance();
    let camera = [view_state.width() / 2.0, view_state.height() / 2.0 + offset];
    [
        x0.min(camera[0]),
        y0.min(camera[1]),
        x1.max(camera[0]),
        y1.max(camera[1]),
    ]
}

/// The rectangle of world pixels a screen box covers on the ground plane, or `None` when none of
/// its corners reaches the ground (it is all sky).
pub(super) fn ground_region(view_state: &ViewState, bounds: [f64; 4]) -> Option<[f64; 4]> {
    ground_corners(view_state, bounds)
        .into_iter()
        .fold(None, |region, [x, y]| {
            Some(match region {
                None => [x, y, x, y],
                Some(r) => [r[0].min(x), r[1].min(y), r[2].max(x), r[3].max(y)],
            })
        })
}

/// The corners of a screen box on the ground plane in world pixels, leaving out those in the
/// sky.
pub(super) fn ground_corners(view_state: &ViewState, bounds: [f64; 4]) -> Vec<[f64; 2]> {
    let Ok(inverted) = view_state.inverted_view_projection() else {
        return Vec::new();
    };
    let mut corners = Vec::with_capacity(4);
    for (x, y) in [
        (bounds[0], bounds[1]),
        (bounds[2], bounds[1]),
        (bounds[2], bounds[3]),
        (bounds[0], bounds[3]),
    ] {
        let Some(point) =
            view_state.window_to_world_at_ground(&Vector2::new(x, y), &inverted, true)
        else {
            continue;
        };
        if point.x.is_finite() && point.y.is_finite() {
            corners.push([point.x, point.y]);
        }
    }
    corners
}

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

/// The tiles at grid level `z` that a region of world pixels at view zoom `zoom` covers, or
/// comes within `margin` world pixels of.
pub(super) fn tiles_in(region: [f64; 4], margin: f64, zoom: Zoom, z: u8) -> Vec<QueryTile> {
    let scale = zoom.scale_to_zoom_level(ZoomLevel::new(z));
    let to_grid = |world: f64| world / TILE_SIZE * scale;
    let (x0, y0) = (to_grid(region[0]), to_grid(region[1]));
    let (x1, y1) = (to_grid(region[2]), to_grid(region[3]));
    let reach = to_grid(margin);
    let (tx0, tx1) = ((x0 - reach).floor() as i64, (x1 + reach).floor() as i64);
    let (ty0, ty1) = ((y0 - reach).floor() as i64, (y1 + reach).floor() as i64);
    let tiles_wide = 1_i64 << z;
    if (tx1 - tx0 + 1) * (ty1 - ty0 + 1) > MAX_TILES {
        return Vec::new();
    }
    let units_per_pixel = EXTENT * scale / TILE_SIZE;
    let mut tiles = Vec::new();
    for ty in ty0.max(0)..=ty1.min(tiles_wide - 1) {
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

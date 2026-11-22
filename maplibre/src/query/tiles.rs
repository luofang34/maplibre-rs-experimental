//! The tiles a query reads and the rectangle it covers in each.

use rustc_hash::FxHashSet;

use super::ground::Footprint;
use crate::{
    coords::{WorldTileCoords, ZoomLevel, EXTENT, TILE_SIZE},
    io::geometry_index::{IndexedGeometry, TileIndex},
    render::resolved_view::ResolvedFrameView,
    tcs::world::World,
};

/// Tiles a single query reads at one zoom before it uses a coarser one.
const MAX_TILES: i64 = 256;
const MAX_QUERY_ZOOM: i32 = 22;

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

/// A resolved query tile together with the loaded source tile and overzoom depth it draws from.
pub(super) struct QueryEntry {
    pub(super) tile: QueryTile,
    pub(super) source_coords: WorldTileCoords,
    pub(super) depth: u8,
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

struct GridWindow {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    tx0: i64,
    tx1: i64,
    ty0: i64,
    ty1: i64,
    scale: f64,
    tiles_wide: i64,
    base_units_per_pixel: f64,
}

fn grid_window(region: [f64; 4], margin: f64, resolved: &ResolvedFrameView, z: u8) -> GridWindow {
    let scale = resolved.zoom.scale_to_zoom_level(ZoomLevel::new(z));
    let to_grid = |world: f64| world / TILE_SIZE * scale;
    let (x0, y0) = (to_grid(region[0]), to_grid(region[1]));
    let (x1, y1) = (to_grid(region[2]), to_grid(region[3]));
    let ws = resolved.world_size.max(1e-9);
    let my0 = (region[1] / ws).clamp(0.0, 1.0);
    let my1 = (region[3] / ws).clamp(0.0, 1.0);
    let mid_my = ((region[1] + region[3]) * 0.5 / ws).clamp(0.0, 1.0);
    let max_scale = resolved
        .globe_scale_at_mercator_y(my0)
        .max(resolved.globe_scale_at_mercator_y(my1))
        .max(resolved.globe_scale_at_mercator_y(mid_my));
    let tiles_wide = 1_i64 << z;
    let reach = to_grid(margin * max_scale).min(tiles_wide as f64);
    let (tx0, tx1) = ((x0 - reach).floor() as i64, (x1 + reach).floor() as i64);
    let (ty0, ty1) = ((y0 - reach).floor() as i64, (y1 + reach).floor() as i64);
    GridWindow {
        x0,
        y0,
        x1,
        y1,
        tx0,
        tx1,
        ty0,
        ty1,
        scale,
        tiles_wide,
        base_units_per_pixel: EXTENT * scale / TILE_SIZE,
    }
}

fn build_query_tile(
    tx: i64,
    ty: i64,
    z: u8,
    w: &GridWindow,
    resolved: &ResolvedFrameView,
) -> QueryTile {
    let canonical = tx.rem_euclid(w.tiles_wide);
    let row_my = ((ty as f64 + 0.5) / w.tiles_wide as f64).clamp(0.0, 1.0);
    let units_per_pixel = w.base_units_per_pixel * resolved.globe_scale_at_mercator_y(row_my);
    QueryTile {
        coords: WorldTileCoords {
            x: canonical as i32,
            y: ty as i32,
            z: ZoomLevel::new(z),
        },
        wrap: tx.div_euclid(w.tiles_wide) as i32,
        local: [
            (w.x0 - tx as f64) * EXTENT,
            (w.y0 - ty as f64) * EXTENT,
            (w.x1 - tx as f64) * EXTENT,
            (w.y1 - ty as f64) * EXTENT,
        ],
        units_per_pixel,
        zoom_level: z,
        origin: [
            tx as f64 * TILE_SIZE / w.scale,
            ty as f64 * TILE_SIZE / w.scale,
        ],
        world_per_unit: TILE_SIZE / w.scale / EXTENT,
        footprint: None,
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
    let w = grid_window(region, margin, resolved, z);
    if (w.tx1 - w.tx0 + 1) * (w.ty1 - w.ty0 + 1) > MAX_TILES {
        return Vec::new();
    }
    let mut tiles = Vec::new();
    for ty in w.ty0.max(0)..=w.ty1.min(w.tiles_wide - 1) {
        for tx in w.tx0..=w.tx1 {
            tiles.push(build_query_tile(tx, ty, z, &w, resolved));
        }
    }
    tiles
}

fn is_strict_descendant(desc: WorldTileCoords, anc: WorldTileCoords) -> bool {
    let dz = u8::from(desc.z).checked_sub(u8::from(anc.z));
    let Some(shift) = dz.filter(|&d| d > 0) else {
        return false;
    };
    (desc.x >> shift) == anc.x && (desc.y >> shift) == anc.y
}

fn emit_subtiles(
    sub_tx: i64,
    sub_ty: i64,
    sub_z: u8,
    src: WorldTileCoords,
    (view_z, region, margin, resolved): (u8, [f64; 4], f64, &ResolvedFrameView),
    out: &mut Vec<QueryEntry>,
) {
    let z_s = u8::from(src.z);
    let target_z = if view_z > sub_z && (view_z - sub_z) <= 3 {
        let shift = view_z - sub_z;
        let w = grid_window(region, margin, resolved, view_z);
        let ix0 = (sub_tx << shift).max(w.tx0);
        let ix1 = (((sub_tx + 1) << shift) - 1).min(w.tx1);
        let iy0 = (sub_ty << shift).max(w.ty0.max(0));
        let iy1 = (((sub_ty + 1) << shift) - 1).min(w.ty1.min(w.tiles_wide - 1));
        let count = (ix1 - ix0 + 1).max(0) * (iy1 - iy0 + 1).max(0);
        if count > 0 && count <= 16 {
            for ty in iy0..=iy1 {
                for tx in ix0..=ix1 {
                    out.push(QueryEntry {
                        tile: build_query_tile(tx, ty, view_z, &w, resolved),
                        source_coords: src,
                        depth: view_z - z_s,
                    });
                }
            }
            return;
        }
        sub_z
    } else {
        sub_z
    };
    let w = grid_window(region, margin, resolved, target_z);
    if sub_ty >= w.ty0.max(0)
        && sub_ty <= w.ty1.min(w.tiles_wide - 1)
        && sub_tx >= w.tx0
        && sub_tx <= w.tx1
    {
        out.push(QueryEntry {
            tile: build_query_tile(sub_tx, sub_ty, target_z, &w, resolved),
            source_coords: src,
            depth: target_z - z_s,
        });
    }
}

fn collect_uncovered_subtrees(
    (cur_tx, cur_ty, cur_z): (i64, i64, u8),
    src: WorldTileCoords,
    loaded: &FxHashSet<WorldTileCoords>,
    max_desc_z: u8,
    ctx: (u8, [f64; 4], f64, &ResolvedFrameView),
    out: &mut Vec<QueryEntry>,
) {
    let w = grid_window(ctx.1, ctx.2, ctx.3, cur_z);
    if cur_ty < w.ty0.max(0)
        || cur_ty > w.ty1.min(w.tiles_wide - 1)
        || cur_tx < w.tx0
        || cur_tx > w.tx1
    {
        return;
    }
    let canonical = WorldTileCoords {
        x: cur_tx.rem_euclid(w.tiles_wide) as i32,
        y: cur_ty as i32,
        z: ZoomLevel::new(cur_z),
    };
    if cur_z > u8::from(src.z) && loaded.contains(&canonical) {
        return;
    }
    let has_desc = cur_z < max_desc_z && loaded.iter().any(|&d| is_strict_descendant(d, canonical));
    if has_desc {
        for dy in 0..2 {
            for dx in 0..2 {
                collect_uncovered_subtrees(
                    (cur_tx * 2 + dx, cur_ty * 2 + dy, cur_z + 1),
                    src,
                    loaded,
                    max_desc_z,
                    ctx,
                    out,
                );
            }
        }
    } else {
        emit_subtiles(cur_tx, cur_ty, cur_z, src, ctx, out);
    }
}

/// Selects all loaded query tiles covering `region` (expanded by `margin`), handling mixed-LOD
/// globe views and regions whose flat tile count exceeds `MAX_TILES`.
pub(super) fn select_query_entries(
    world: &World,
    region: [f64; 4],
    margin: f64,
    resolved: &ResolvedFrameView,
) -> Vec<QueryEntry> {
    let zoom = resolved.zoom;
    let top = (zoom.value().floor() as i32 + 1).clamp(0, MAX_QUERY_ZOOM);
    let view_z = (zoom.value().floor() as i32).clamp(0, MAX_QUERY_ZOOM) as u8;

    if resolved.globe_transition() == 0.0 {
        if let Some(z) = (0..=top).rev().find(|&z| {
            tiles_in(region, margin, resolved, z as u8)
                .iter()
                .any(|tile| {
                    world
                        .tiles
                        .geometry_index
                        .tile_indexes(&tile.coords)
                        .is_some()
                })
        }) {
            let z_u8 = z as u8;
            let tile_z = z_u8.max(view_z);
            let candidates = tiles_in(region, margin, resolved, tile_z);
            if !candidates.is_empty() {
                let depth = tile_z - z_u8;
                return candidates
                    .into_iter()
                    .filter_map(|tile| {
                        let source_coords = WorldTileCoords {
                            x: tile.coords.x >> depth,
                            y: tile.coords.y >> depth,
                            z: ZoomLevel::new(z_u8),
                        };
                        world
                            .tiles
                            .geometry_index
                            .tile_indexes(&source_coords)
                            .map(|_| QueryEntry {
                                tile,
                                source_coords,
                                depth,
                            })
                    })
                    .collect();
            }
        }
    }

    let top_u8 = top as u8;
    let loaded: FxHashSet<WorldTileCoords> = world
        .tiles
        .geometry_index
        .indexed_coords()
        .filter(|c| u8::from(c.z) <= top_u8)
        .collect();
    if loaded.is_empty() {
        return Vec::new();
    }
    let mut ordered: Vec<WorldTileCoords> = loaded.iter().copied().collect();
    ordered.sort_by_key(|c| (u8::from(c.z), c.y, c.x));

    let mut out = Vec::new();
    let ctx = (view_z, region, margin, resolved);
    for src in ordered {
        let z_s = u8::from(src.z);
        let w_s = grid_window(region, margin, resolved, z_s);
        let sy = i64::from(src.y);
        if sy < w_s.ty0.max(0) || sy > w_s.ty1.min(w_s.tiles_wide - 1) {
            continue;
        }
        let max_desc_z = loaded
            .iter()
            .filter(|&&d| is_strict_descendant(d, src))
            .map(|d| u8::from(d.z))
            .max()
            .unwrap_or(z_s);
        let min_w = (w_s.tx0 - w_s.tiles_wide + 1 - i64::from(src.x)).div_euclid(w_s.tiles_wide);
        let max_w = (w_s.tx1 - i64::from(src.x)).div_euclid(w_s.tiles_wide);
        for wrap in min_w..=max_w {
            let tx = i64::from(src.x) + wrap * w_s.tiles_wide;
            if tx < w_s.tx0 || tx > w_s.tx1 {
                continue;
            }
            if max_desc_z > z_s {
                collect_uncovered_subtrees((tx, sy, z_s), src, &loaded, max_desc_z, ctx, &mut out);
            } else {
                emit_subtiles(tx, sy, z_s, src, ctx, &mut out);
            }
        }
    }
    out
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
        TileIndex::LazyMvt(lazy) => lazy
            .geometries()
            .iter()
            .filter(|geometry| {
                use rstar::Envelope;
                geometry.bounds.intersects(&window)
            })
            .collect(),
    }
}

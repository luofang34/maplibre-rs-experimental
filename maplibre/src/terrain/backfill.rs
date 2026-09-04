//! Replaces replicated DEM borders with the true edge samples of loaded neighbours.
//!
//! A tile decodes with its border copied from its own edge so interpolation is continuous
//! straight away; once a neighbour loads, both tiles take each other's edge samples, as GL JS
//! `backfillDEM` does, so the meshes meet without a step along the shared edge.

use crate::{
    coords::{WorldTileCoords, ZoomLevel},
    tcs::tiles::Tiles,
    terrain::DemTileComponent,
};

const NEIGHBOUR_OFFSETS: [(i32, i32); 8] = [
    (-1, -1),
    (0, -1),
    (1, -1),
    (-1, 0),
    (1, 0),
    (-1, 1),
    (0, 1),
    (1, 1),
];

/// Neighbour coordinates of a tile, wrapping across the antimeridian and skipping the poles.
pub fn neighbours(coords: WorldTileCoords) -> Vec<(WorldTileCoords, (i32, i32))> {
    let zoom = u8::from(coords.z);
    let tiles_per_axis = 1_i64 << zoom;
    NEIGHBOUR_OFFSETS
        .iter()
        .filter_map(|&(dx, dy)| {
            let y = i64::from(coords.y) + i64::from(dy);
            if y < 0 || y >= tiles_per_axis {
                return None;
            }
            let x = (i64::from(coords.x) + i64::from(dx)).rem_euclid(tiles_per_axis);
            let neighbour = WorldTileCoords {
                x: x as i32,
                y: y as i32,
                z: ZoomLevel::new(zoom),
            };
            (neighbour != coords).then_some((neighbour, (dx, dy)))
        })
        .collect()
}

/// Fills the borders between a freshly loaded tile and every loaded neighbour, both ways.
pub fn backfill_neighbours(tiles: &mut Tiles, coords: WorldTileCoords) {
    for (neighbour, (dx, dy)) in neighbours(coords) {
        fill_border(tiles, coords, neighbour, dx, dy);
        fill_border(tiles, neighbour, coords, -dx, -dy);
    }
}

/// Copies `source`'s edge into `target`'s border, where `source` sits at `(dx, dy)` from it.
fn fill_border(
    tiles: &mut Tiles,
    target: WorldTileCoords,
    source: WorldTileCoords,
    dx: i32,
    dy: i32,
) {
    let already_filled = matches!(
        tiles.query::<&DemTileComponent>(target),
        Some(DemTileComponent::Loaded(dem)) if dem.backfilled.contains(&source)
    );
    if already_filled {
        return;
    }
    let Some(DemTileComponent::Loaded(source_dem)) = tiles.query::<&DemTileComponent>(source)
    else {
        return;
    };
    let samples = source_dem.tile.edge_samples(dx, dy);
    let source_dim = source_dem.tile.dim();
    let Some(DemTileComponent::Loaded(target_dem)) =
        tiles.query_mut::<&mut DemTileComponent>(target)
    else {
        return;
    };
    if target_dem.tile.dim() != source_dim {
        tracing::warn!(
            %target,
            %source,
            "DEM neighbours differ in size; the replicated border stays"
        );
        return;
    }
    target_dem.tile.fill_border(dx, dy, &samples);
    target_dem.backfilled.insert(source);
    target_dem.revision = target_dem.revision.wrapping_add(1);
}

#[cfg(test)]
mod tests;

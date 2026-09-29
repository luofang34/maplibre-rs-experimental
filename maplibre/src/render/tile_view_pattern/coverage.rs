//! Complete, disjoint source coverage shared by screen and terrain rendering.

use super::{HasTile, COMPLETE_CHILDREN_SEARCH_DEPTH};
use crate::{coords::WorldTileCoords, projection::tile_covering::covers, tcs::world::World};

/// The tile itself, complete descendants, or its nearest loaded ancestor.
pub(crate) fn loaded_cover(
    sources: &impl HasTile,
    coords: WorldTileCoords,
    world: &World,
) -> Option<Vec<WorldTileCoords>> {
    if sources.has_tile(coords, world) {
        Some(vec![coords])
    } else {
        sources
            .get_complete_children(coords, world, COMPLETE_CHILDREN_SEARCH_DEPTH)
            .or_else(|| {
                sources
                    .get_available_parent(coords, world)
                    .map(|parent| vec![parent])
            })
    }
}

/// A disjoint covering, or no replacement while some part of the target is missing.
pub(crate) fn complete_cover(
    target: WorldTileCoords,
    tiles: Vec<WorldTileCoords>,
) -> Option<Vec<WorldTileCoords>> {
    target.build_quad_key()?;
    let disjoint = disjoint_tiles(target, tiles);
    covers_target(target, &disjoint).then_some(disjoint)
}

pub(super) fn disjoint_tiles(
    target: WorldTileCoords,
    mut tiles: Vec<WorldTileCoords>,
) -> Vec<WorldTileCoords> {
    tiles.retain(|tile| {
        tile.build_quad_key().is_some() && (covers(*tile, target) || covers(target, *tile))
    });
    tiles.sort_by_key(|tile| tile.z);
    let mut disjoint = Vec::new();
    for tile in tiles {
        // Drawing both an ancestor and a descendant would blend translucent layers twice.
        if !disjoint.iter().any(|parent| covers(*parent, tile)) {
            disjoint.push(tile);
        }
    }
    disjoint
}

/// Checks the union, so duplicates and unrelated tiles cannot fill a missing quadrant.
pub(crate) fn covers_target(target: WorldTileCoords, tiles: &[WorldTileCoords]) -> bool {
    if target.build_quad_key().is_none() {
        return false;
    }
    let tiles: Vec<_> = tiles
        .iter()
        .copied()
        .filter(|tile| tile.build_quad_key().is_some())
        .collect();
    covers_subtree(target, &tiles)
}

fn covers_subtree(target: WorldTileCoords, tiles: &[WorldTileCoords]) -> bool {
    if tiles.iter().any(|tile| covers(*tile, target)) {
        return true;
    }
    let descendants: Vec<_> = tiles
        .iter()
        .copied()
        .filter(|tile| covers(target, *tile))
        .collect();
    // Recursion stops at the deepest supplied tile, including the maximum supported zoom.
    !descendants.is_empty()
        && target
            .get_children()
            .into_iter()
            .all(|child| covers_subtree(child, &descendants))
}

#[cfg(test)]
mod tests;

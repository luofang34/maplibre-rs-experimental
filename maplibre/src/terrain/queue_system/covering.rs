//! Fits terrain coverage to the texture budget by coarsening distant tiles.
#[cfg(test)]
use crate::projection::tile_covering::covers;
use crate::{coords::WorldTileCoords, tcs::world::World};

pub(super) fn bounded_covering(
    tiles: impl Iterator<Item = WorldTileCoords>,
    limit: usize,
) -> Vec<WorldTileCoords> {
    crate::projection::tile_covering::coarsen(tiles, limit, 0)
}

#[derive(Default)]
struct CachedCovering {
    tiles: Vec<WorldTileCoords>,
    limit: usize,
    covering: Vec<WorldTileCoords>,
}

pub(super) fn for_frame(
    world: &mut World,
    tiles: Vec<WorldTileCoords>,
    limit: usize,
) -> Vec<WorldTileCoords> {
    let cached = world.resources.get_or_init_mut::<CachedCovering>();
    if cached.tiles != tiles || cached.limit != limit {
        cached.covering = bounded_covering(tiles.iter().copied(), limit);
        cached.tiles = tiles;
        cached.limit = limit;
    }
    cached.covering.clone()
}

#[cfg(test)]
mod tests;

/// Keep only the nearest presented fallback for a target awaiting its first draw.
pub(super) fn retained_textures(
    targets: impl Iterator<Item = WorldTileCoords>,
    present: impl Fn(WorldTileCoords) -> bool,
) -> std::collections::HashSet<WorldTileCoords> {
    let targets: Vec<_> = targets.collect();
    let mut kept = targets
        .iter()
        .copied()
        .collect::<std::collections::HashSet<_>>();
    for target in targets {
        if present(target) {
            continue;
        }
        let mut current = target;
        while let Some(parent) = current.get_parent() {
            if present(parent) {
                kept.insert(parent);
                break;
            }
            current = parent;
        }
    }
    kept
}

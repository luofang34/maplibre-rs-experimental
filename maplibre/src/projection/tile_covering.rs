//! Coverage-preserving coarsening for a bounded tile working set.
use crate::coords::WorldTileCoords;
use std::collections::HashMap;

pub(crate) fn coarsen(
    tiles: impl Iterator<Item = WorldTileCoords>,
    limit: usize,
    min_zoom: u8,
) -> Vec<WorldTileCoords> {
    let mut kept: Vec<_> = tiles.collect();
    let limit = limit.max(1);
    while kept.len() > limit {
        let Some(parent) = coarsening_parent(&kept, min_zoom) else {
            break;
        };
        // The covering is nearest first. Keep the parent's first descendant's priority.
        let mut inserted = false;
        kept = kept
            .into_iter()
            .filter_map(|tile| {
                if covers(parent, tile) {
                    if inserted {
                        return None;
                    }
                    inserted = true;
                    Some(parent)
                } else {
                    Some(tile)
                }
            })
            .collect();
    }
    kept
}

fn coarsening_parent(tiles: &[WorldTileCoords], min_zoom: u8) -> Option<WorldTileCoords> {
    let mut parents = HashMap::new();
    for (index, tile) in tiles.iter().enumerate() {
        if let Some(parent) = tile
            .get_parent()
            .filter(|parent| u8::from(parent.z) >= min_zoom)
        {
            let entry = parents.entry(parent).or_insert((index, 0_usize));
            entry.1 += 1;
        }
    }
    // Different-zoom leaves share ancestors without being direct siblings.
    for (index, tile) in tiles.iter().enumerate() {
        let mut ancestor = tile.get_parent();
        while let Some(coords) = ancestor {
            if let Some((first, _)) = parents.get_mut(&coords) {
                *first = (*first).min(index);
            }
            ancestor = coords.get_parent();
        }
    }
    parents
        .into_iter()
        .map(|(parent, (first, count))| (parent, first, count))
        .max_by_key(|(parent, first, count)| (*count > 1, *first, *parent))
        .map(|(parent, _, _)| parent)
}

pub(crate) fn covers(parent: WorldTileCoords, tile: WorldTileCoords) -> bool {
    let delta = i32::from(u8::from(tile.z)) - i32::from(u8::from(parent.z));
    delta >= 0 && (tile.x >> delta) == parent.x && (tile.y >> delta) == parent.y
}

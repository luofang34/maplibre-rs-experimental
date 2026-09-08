//! Split and merge hysteresis without delaying newly visible ground.
use crate::coords::{TileCoords, WorldTileCoords};
use std::collections::HashSet;

const ZOOM_MARGIN: f64 = 0.15;

#[derive(Default, Debug)]
pub(crate) struct LodHistory {
    leaves: HashSet<WorldTileCoords>,
    refined: HashSet<WorldTileCoords>,
}

impl LodHistory {
    pub(crate) fn new(tiles: &[WorldTileCoords]) -> Self {
        let mut refined = HashSet::new();
        for tile in tiles {
            let mut parent = tile.get_parent();
            while let Some(tile) = parent {
                refined.insert(tile);
                parent = tile.get_parent();
            }
        }
        Self {
            leaves: tiles.iter().copied().collect(),
            refined,
        }
    }

    pub(crate) fn bias(&self, tile: TileCoords) -> f64 {
        let tile = WorldTileCoords {
            x: tile.x as i32,
            y: tile.y as i32,
            z: tile.z,
        };
        if self.refined.contains(&tile) {
            ZOOM_MARGIN
        } else if self.leaves.contains(&tile) {
            -ZOOM_MARGIN
        } else {
            0.0
        }
    }
}

#[cfg(test)]
mod tests;

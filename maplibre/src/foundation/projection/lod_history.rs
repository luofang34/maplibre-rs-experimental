//! Split and merge hysteresis without delaying newly visible ground.
use rustc_hash::FxHashSet;

use crate::coords::{TileCoords, WorldTileCoords};

const ZOOM_MARGIN: f64 = 0.15;

#[derive(Clone, Default, Debug)]
pub(crate) struct LodHistory {
    leaves: FxHashSet<WorldTileCoords>,
    refined: FxHashSet<WorldTileCoords>,
}

impl LodHistory {
    pub(crate) fn is_empty(&self) -> bool {
        self.leaves.is_empty()
    }

    pub(crate) fn rebuild(&mut self, tiles: &[WorldTileCoords]) {
        // A tile is judged by its canonical coordinates, whichever copy of the world it was
        // drawn in.
        self.leaves.clear();
        self.leaves.extend(
            tiles
                .iter()
                .filter_map(|tile| tile.wrapped().map(|(canonical, _)| canonical)),
        );
        self.refined.clear();
        for tile in &self.leaves {
            let mut parent = tile.get_parent();
            while let Some(tile) = parent {
                self.refined.insert(tile);
                parent = tile.get_parent();
            }
        }
    }

    pub(crate) fn new(tiles: &[WorldTileCoords]) -> Self {
        let mut history = Self::default();
        history.rebuild(tiles);
        history
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

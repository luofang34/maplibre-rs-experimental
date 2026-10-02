//! Raster tiles a zoom leaves behind while the tiles that replace them fade in.
//!
//! GL JS keeps a source's loaded descendants of a newly shown tile and draws each mixed with
//! that tile, by the share of `raster-fade-duration` that has passed. For opaque tiles this is
//! the new tile drawn whole with the departing ones over it at the share still to go.

use std::collections::{HashMap, HashSet};

use crate::coords::WorldTileCoords;

/// The departing tiles of each raster source, by source name.
#[derive(Default)]
pub struct RasterCrossFade {
    sources: HashMap<String, DepartingTiles>,
}

/// Tiles of a source that are fading out, and how opaque they still are.
pub struct DepartingTiles {
    /// The departing tiles; only descendants of a tile the view shows are drawn.
    pub tiles: HashSet<WorldTileCoords>,
    /// Opacity left to the departing tiles, from one when the fade starts to zero when it ends.
    pub opacity: f32,
}

impl RasterCrossFade {
    /// Fades `tiles` of `source` out, with `opacity` of them still showing.
    pub fn insert(&mut self, source: &str, tiles: HashSet<WorldTileCoords>, opacity: f32) {
        self.sources.insert(
            source.to_owned(),
            DepartingTiles {
                tiles,
                opacity: opacity.clamp(0.0, 1.0),
            },
        );
    }

    /// The departing tiles of `source`, if it is fading.
    pub fn departing(&self, source: Option<&str>) -> Option<&DepartingTiles> {
        self.sources.get(source?)
    }

    /// Whether `coords` of `source` is fading out.
    pub fn is_departing(&self, source: Option<&str>, coords: &WorldTileCoords) -> bool {
        self.departing(source)
            .is_some_and(|departing| departing.tiles.contains(coords))
    }
}

#[cfg(test)]
mod tests;

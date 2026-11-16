//! Map-owned resource and tile stores passed to systems during a frame.

use crate::{
    resources::Resources,
    tiles::{TileGeometryStore, Tiles},
};

/// Mutable state of one map; its stores are independently borrowable by a system.
pub struct World<G: TileGeometryStore = ()> {
    /// Map-wide typed values, including renderer resources and frame state.
    pub resources: Resources,
    /// Tile records, components and feature-query geometry.
    pub tiles: Tiles<G>,
}

impl<G: TileGeometryStore> Default for World<G> {
    fn default() -> Self {
        Self {
            resources: Resources::default(),
            tiles: Tiles::default(),
        }
    }
}

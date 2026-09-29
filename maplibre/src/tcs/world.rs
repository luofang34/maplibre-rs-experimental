//! Map-owned resource and tile stores passed to systems during a frame.

use crate::tcs::{resources::Resources, tiles::Tiles};

/// Mutable state of one map; its stores are independently borrowable by a system.
#[derive(Default)]
pub struct World {
    /// Map-wide typed values, including renderer resources and frame state.
    pub resources: Resources,
    /// Tile records, components and feature-query geometry.
    pub tiles: Tiles,
}

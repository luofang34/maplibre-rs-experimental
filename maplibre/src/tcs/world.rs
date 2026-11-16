//! Map-owned resource and tile stores passed to systems during a frame.

use crate::io::geometry_index::GeometryIndex;

/// Mutable state of one map; its stores are independently borrowable by a system.
pub type World = maplibre_tcs::world::World<GeometryIndex>;

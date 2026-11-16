//! Tile records, their typed CPU components and rendered-feature query geometry.

pub use maplibre_tcs::tiles::{
    ComponentQuery, ComponentQueryMut, Tile, TileComponent, TileGeometryStore, TileSpawnResult,
};

use crate::io::geometry_index::GeometryIndex;

/// Owns tile records, typed components and their feature-query index.
pub type Tiles = maplibre_tcs::tiles::Tiles<GeometryIndex>;

#[cfg(test)]
mod tests;

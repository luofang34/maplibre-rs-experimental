//! Drawn terrain built from DEM tiles a host holds, so lines of sight and the drawn ground can
//! be asked without a map drawing a frame.
//!
//! The host names the terrain tiles a frame would draw and hands over the DEM tiles it has
//! decoded; which DEM tile each drawn tile samples, one zoom coarser and at most the source's
//! maxzoom, or the nearest loaded ancestor, is decided here as the map decides it.

use std::collections::BTreeSet;

use super::DrawnTerrain;
use crate::{
    coords::WorldTileCoords,
    projection::body::Body,
    tcs::tiles::Tiles,
    terrain::{
        coverage::TerrainCoverageIndex, dem::DemTile, request_system::dem_tile_coords,
        source::DemSource, DemTileComponent, LoadedDem,
    },
};

/// Why [`LoadedGround::new`] cannot build the ground.
#[derive(Debug, thiserror::Error)]
pub enum LoadedGroundError {
    /// A tile's coordinates lie outside the tile pyramid at its zoom.
    #[error("tile {coords:?} lies outside the tile pyramid")]
    InvalidTile {
        /// The tile's coordinates.
        coords: WorldTileCoords,
    },
}

/// Terrain tiles and the DEM behind them, as a frame would draw them.
pub struct LoadedGround {
    pub(crate) tiles: Tiles,
    pub(crate) index: TerrainCoverageIndex,
}

impl LoadedGround {
    /// The DEM tiles the terrain tiles `rendered` sample, in tile order: the tiles to decode
    /// for [`new`](Self::new). A tile coarser than the source's minzoom samples none.
    pub fn dem_tiles_for(
        rendered: impl IntoIterator<Item = WorldTileCoords>,
        dem: &DemSource,
    ) -> Vec<WorldTileCoords> {
        rendered
            .into_iter()
            .filter_map(|coords| dem_tile_coords(coords, dem.minzoom, dem.maxzoom))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// The terrain tiles `rendered` drawn from the decoded `dem_tiles` of `dem`, the source
    /// [`dem_source`](crate::terrain::source::dem_source) finds in the style. A drawn tile
    /// whose DEM tile, and every ancestor of it, is missing from `dem_tiles` is unknown
    /// ground, as a frame draws it before its DEM arrives.
    pub fn new(
        rendered: impl IntoIterator<Item = WorldTileCoords>,
        dem_tiles: impl IntoIterator<Item = (WorldTileCoords, DemTile)>,
        dem: &DemSource,
    ) -> Result<Self, LoadedGroundError> {
        let mut tiles = Tiles::default();
        for (coords, tile) in dem_tiles {
            tiles
                .spawn_mut(coords)
                .ok_or(LoadedGroundError::InvalidTile { coords })?
                .insert(DemTileComponent::Loaded(LoadedDem::new(tile)));
        }
        let rendered = rendered
            .into_iter()
            .map(|coords| {
                coords
                    .build_quad_key()
                    .map(|_| coords)
                    .ok_or(LoadedGroundError::InvalidTile { coords })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let index = TerrainCoverageIndex::build(rendered, &tiles, dem);
        Ok(Self { tiles, index })
    }

    /// The ground on `body`, to follow lines of sight across and ask the drawn ground of.
    pub fn terrain(&self, body: Body) -> DrawnTerrain<'_> {
        DrawnTerrain {
            index: &self.index,
            tiles: &self.tiles,
            body,
        }
    }

    /// The index of the drawn tiles, which also answers the best loaded DEM.
    pub fn index(&self) -> &TerrainCoverageIndex {
        &self.index
    }

    /// The decoded DEM tiles, as [`TerrainCoverageIndex::loaded_ground_cached`] reads them.
    pub fn tiles(&self) -> &Tiles {
        &self.tiles
    }
}

#[cfg(test)]
mod tests;

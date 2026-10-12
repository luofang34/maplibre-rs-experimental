//! The height of the drawn ground at a location.

use cgmath::{Point2, Vector3};

use super::{
    lat_lon_to_mercator, mesh, mesh::Cell, DrawnTerrain, MAX_MERCATOR_Y, MAX_VALID_LATITUDE,
};
use crate::{
    coords::{LatLon, WorldTileCoords},
    projection::globe::lat_lon_to_unit_sphere,
};

/// The frame draws a tile at a location whose DEM has not loaded, so the ground's height
/// there is not known yet; it is not sea level.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("rendered terrain tile {tile:?} has no DEM loaded yet")]
pub struct TerrainUnknown {
    /// The rendered tile without its DEM.
    pub tile: WorldTileCoords,
}

impl DrawnTerrain<'_> {
    /// Height in drawn metres of the ground the frame draws at a location: where the line up
    /// from the body's center meets the drawn triangles, a polar cap's fan beyond the last row
    /// of tiles. `Ok(None)` where no rendered tile lies, and [`TerrainUnknown`] where one does
    /// but its DEM has not loaded.
    pub fn ground_at(&self, location: LatLon) -> Result<Option<f64>, TerrainUnknown> {
        let up = lat_lon_to_unit_sphere(location);
        let mercator = lat_lon_to_mercator(location);
        let Some(cell) = self.cell_at(mercator, location.latitude.abs() > MAX_VALID_LATITUDE)
        else {
            return Ok(None);
        };
        let triangles = self.triangles(cell);
        if triangles.is_empty() {
            return Err(TerrainUnknown { tile: cell.tile });
        }
        Ok(triangles
            .into_iter()
            .filter_map(|triangle| mesh::intersect(Vector3::new(0.0, 0.0, 0.0), up, triangle))
            .reduce(f64::max)
            .map(|radius| (radius - 1.0) * self.body.radius_meters))
    }

    /// The drawn cell over `mercator`: the rendered tile's grid cell, or under a polar cap the
    /// fan of the last tile in its column.
    pub(super) fn cell_at(&self, mercator: Point2<f64>, polar: bool) -> Option<Cell> {
        if polar {
            let edge = if mercator.y < 0.5 {
                0.0
            } else {
                MAX_MERCATOR_Y
            };
            return self
                .index
                .rendered_tile_at(mercator.x, edge)
                .map(|tile| Cell {
                    row: None,
                    ..Cell::of(tile, mercator)
                });
        }
        self.index
            .rendered_tile_at(mercator.x, mercator.y)
            .map(|tile| Cell::of(tile, mercator))
    }
}

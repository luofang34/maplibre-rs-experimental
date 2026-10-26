//! How high the ground is at a place, as the map knows it: for picking, standing markers on
//! the ground and keeping cameras above it.

use crate::{
    context::MapContext,
    coords::{LatLon, WorldCoords, Zoom, TILE_SIZE},
    projection::body::Body,
    terrain::{
        coverage::TerrainCoverageIndex,
        sightline::{DrawnTerrain, TerrainUnknown},
        DemDetail,
    },
};

impl MapContext {
    /// Ground height in metres at a location from the finest DEM tile loaded so far, drawn or
    /// not; `None` without terrain or before any tile covering the location has arrived. The
    /// answer improves as finer tiles load, so read it again rather than keeping it.
    pub fn loaded_ground_cached(&self, position: LatLon) -> Option<f64> {
        let index = self.world.resources.get::<TerrainCoverageIndex>()?;
        let mercator = WorldCoords::from_lat_lon(position, Zoom::new(0.0));
        index.loaded_ground_cached(
            &self.world.tiles,
            mercator.x / TILE_SIZE,
            mercator.y / TILE_SIZE,
            DemDetail::Finest,
        )
    }

    /// Height of the ground the last frame drew at a location, for converting between the
    /// screen and the ground: `Ok(None)` without terrain or outside the drawn tiles, and
    /// [`TerrainUnknown`] over a drawn tile whose DEM has not loaded. A DEM loaded elsewhere
    /// in the cache does not stand in for it.
    pub fn drawn_ground_cached(&self, position: LatLon) -> Result<Option<f64>, TerrainUnknown> {
        self.drawn_terrain(self.view_state.body())
            .map_or(Ok(None), |terrain| terrain.ground_at(position))
    }

    /// The terrain the last frame drew, for following lines of sight across it with
    /// [`target_view`](crate::terrain::sightline::target_view); `None` without terrain or
    /// before any of it is drawn. `body` is the camera's
    /// ([`GlobeCameraState::body`](crate::projection::globe::camera::GlobeCameraState::body)).
    pub fn drawn_terrain(&self, body: Body) -> Option<DrawnTerrain<'_>> {
        let index = self
            .world
            .resources
            .get::<TerrainCoverageIndex>()
            .filter(|index| !index.is_empty())?;
        Some(DrawnTerrain {
            index,
            tiles: &self.world.tiles,
            body,
        })
    }
}

#[cfg(all(test, feature = "headless"))]
mod tests;

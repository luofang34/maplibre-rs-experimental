//! Frames for a head-mounted display, drawn through the offscreen map.

use thiserror::Error;

use crate::{
    coords::{LatLon, WorldCoords, Zoom, TILE_SIZE},
    headless::map::HeadlessMap,
    render::{eventually::Eventually, frame_input::ViewSource, resource::TextureView, xr::XrFrame},
    schedule::StageError,
    terrain::coverage::TerrainCoverageIndex,
};

/// Why a frame for a head-mounted display could not be drawn.
#[derive(Error, Debug)]
pub enum XrFrameError {
    /// An eye's pose has no inverse, so it looks from nowhere.
    #[error("eye {index} has a singular pose")]
    SingularEye {
        /// Position of the eye in the frame.
        index: usize,
    },
    /// A stage of the schedule failed while drawing an eye.
    #[error("eye {index} could not be drawn")]
    Eye {
        /// Position of the eye in the frame.
        index: usize,
        /// The stage failure.
        #[source]
        source: StageError,
    },
}

impl HeadlessMap {
    /// Draws every eye of the frame into its own target, from the placement the host chose.
    ///
    /// Each eye runs the whole schedule with its own view, so tile requests, terrain coverage
    /// and the tile covering follow that eye's frustum rather than an average of the pair.
    /// An eye without a colour target draws into the map's own texture, which then holds
    /// the last such eye.
    pub fn run_xr_frame(&mut self, frame: XrFrame) -> Result<(), XrFrameError> {
        for (index, eye) in frame.eyes.into_iter().enumerate() {
            let view = frame
                .placement
                .view_from(eye.world_from_eye, eye.frustum)
                .ok_or(XrFrameError::SingularEye { index })?;
            let input = self.frame_input_mut();
            input.timestamp = frame.timestamp;
            input.view = ViewSource::External(view);
            input.request_overscan = frame.request_overscan;

            let resources = &mut self.map_context.renderer.resources;
            if let Some(color) = eye.target.color {
                resources.render_target = Eventually::Initialized(TextureView::from(color));
            }
            resources.eye_depth_target = eye.target.depth;
            let result = self.schedule.run_once(&mut self.map_context);
            let resources = &mut self.map_context.renderer.resources;
            resources.eye_depth_target = None;
            // The graph runner takes the target after a frame; a failed frame must not leave
            // the eye's texture behind for the next one.
            resources.render_target.take();
            result.map_err(|source| XrFrameError::Eye { index, source })?;
        }
        Ok(())
    }

    /// Terrain elevation in metres at a location, from the DEM tiles loaded so far; `None`
    /// without terrain or before a tile covering the location arrived. A host stands its
    /// scene on it, so a height above the anchor is a height above the ground.
    pub fn terrain_elevation_at(&self, position: LatLon) -> Option<f64> {
        let world = &self.map_context.world;
        let index = world.resources.get::<TerrainCoverageIndex>()?;
        let mercator = WorldCoords::from_lat_lon(position, Zoom::new(0.0));
        index.elevation_at(&world.tiles, mercator.x / TILE_SIZE, mercator.y / TILE_SIZE)
    }
}

#[cfg(test)]
mod tests;

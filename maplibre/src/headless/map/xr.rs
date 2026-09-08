//! Frames for a head-mounted display, drawn through the offscreen map.

use thiserror::Error;

use crate::{
    coords::{LatLon, WorldCoords, Zoom, TILE_SIZE},
    headless::map::HeadlessMap,
    projection::ProjectionType,
    render::{
        eventually::Eventually,
        eye_covering::EyeInFrame,
        frame_input::{frame_input_system, ViewSource},
        resource::TextureView,
        xr::{PrefetchView, ScenePlacement, XrEye, XrFrame},
        RenderStageLabel,
    },
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
    /// Tile ingestion runs once, before the first eye. Later eyes keep its tile content and
    /// terrain refinement while updating their own projection and render targets.
    /// An eye without a colour target draws into the map's own texture, which then holds
    /// the last such eye.
    pub fn run_xr_frame(&mut self, frame: XrFrame) -> Result<(), XrFrameError> {
        self.map_context
            .view_state
            .set_opaque_environment(frame.opaque_environment);
        self.set_prefetch(frame.prefetch.as_ref(), frame.eyes.first());
        let resources = &mut self.map_context.world.resources;
        let frame_number = resources
            .get::<EyeInFrame>()
            .map_or(0, |eye| eye.frame.wrapping_add(1));
        let views = frame
            .eyes
            .iter()
            .enumerate()
            .map(|(index, eye)| {
                frame
                    .placement
                    .view_from(eye.world_from_eye, eye.frustum)
                    .ok_or(XrFrameError::SingularEye { index })
            })
            .collect::<Result<Vec<_>, _>>()?;
        for (index, (eye, view)) in frame.eyes.into_iter().zip(views).enumerate() {
            // The first eye selects the frame's tiles; the others draw the same ones.
            self.map_context.world.resources.insert(EyeInFrame {
                index,
                frame: frame_number,
            });
            let input = self.frame_input_mut();
            input.timestamp = frame.timestamp;
            input.view = ViewSource::External(view);
            input.request_overscan = frame.request_overscan;

            let resources = &mut self.map_context.renderer.resources;
            if let Some(color) = eye.target.color {
                resources.render_target = Eventually::Initialized(TextureView::from(color));
            }
            resources.eye_depth_target = eye.target.depth;
            let result = if index == 0 {
                self.schedule.run_once(&mut self.map_context)
            } else {
                frame_input_system(&mut self.map_context)
                    .map_err(StageError::from)
                    .and_then(|()| {
                        self.schedule.run_stages(&mut self.map_context, |label| {
                            label != &RenderStageLabel::Extract as &dyn crate::schedule::StageLabel
                        })
                    })
            };
            let resources = &mut self.map_context.renderer.resources;
            resources.eye_depth_target = None;
            // The graph runner takes the target after a frame; a failed frame must not leave
            // the eye's texture behind for the next one.
            resources.render_target.take();
            if let Err(source) = result {
                self.map_context.world.resources.insert(EyeInFrame {
                    index: 0,
                    frame: frame_number,
                });
                return Err(XrFrameError::Eye { index, source });
            }
        }
        self.map_context.world.resources.insert(EyeInFrame {
            index: 0,
            frame: frame_number,
        });
        Ok(())
    }

    /// Keeps the view the frame's first eye would have from `placement`, so the request
    /// systems can cover it, or clears it.
    fn set_prefetch(&mut self, placement: Option<&ScenePlacement>, eye: Option<&XrEye>) {
        // The anchor's altitude follows the terrain as DEM tiles arrive; a destination
        // whose only change is that is the same destination.
        let same_destination = |kept: &ScenePlacement, wanted: &ScenePlacement| {
            kept.world_from_scene == wanted.world_from_scene
                && kept.anchor.position == wanted.anchor.position
        };
        let resources = &mut self.map_context.world.resources;
        let kept = resources.get::<PrefetchView>().is_some_and(|prefetch| {
            match (prefetch.placement.as_ref(), placement) {
                (Some(kept), Some(wanted)) => same_destination(kept, wanted),
                (None, None) => true,
                _ => false,
            }
        });
        if kept {
            return;
        }
        let view = placement
            .zip(eye)
            .and_then(|(placement, eye)| placement.view_from(eye.world_from_eye, eye.frustum));
        let view_state = view.and_then(|view| {
            let projection = self
                .map_context
                .style
                .projection
                .as_ref()
                .map_or(ProjectionType::Mercator, |specification| {
                    specification.projection_type.clone()
                });
            let mut ahead = self.map_context.view_state.clone();
            ahead.set_external_view(view, &projection).ok()?;
            Some(ahead)
        });
        self.map_context.world.resources.insert(PrefetchView {
            view_state,
            placement: placement.copied(),
        });
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

#[cfg(test)]
#[path = "xr/regression/tests.rs"]
mod regression;

#[cfg(test)]
#[path = "xr/surface/tests.rs"]
mod surface;

#[cfg(test)]
#[path = "xr/poles/tests.rs"]
mod poles;

//! Style, tile storage, view and renderer state for a map frame.

#![deny(missing_docs)]

use crate::{
    coords::LatLon,
    projection::globe::scale::MERCATOR_LATITUDE_LIMIT,
    render::{view_state::ViewState, Renderer},
    style::Style,
    tcs::world::World,
};

/// Names one camera move; a later move has a different name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FlightId(pub(crate) u64);

/// How a camera move stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlightOutcome {
    /// The camera is on its way.
    Flying,
    /// The camera reached the destination.
    Arrived,
    /// Something else moved the camera, the host stopped the move, or the camera could not be
    /// placed on the way, so the move ended where the camera was.
    Interrupted,
    /// A later move has finished since, so this one's outcome is no longer kept.
    Superseded,
}

/// Returns `style.center` as [`LatLon`] with latitude clamped to [`MERCATOR_LATITUDE_LIMIT`].
pub(crate) fn clamped_style_center(style: &Style) -> LatLon {
    let [lon, lat] = style.center.unwrap_or_default();
    LatLon::new(
        lat.clamp(-MERCATOR_LATITUDE_LIMIT, MERCATOR_LATITUDE_LIMIT),
        lon,
    )
}

/// Mutable frame state shared by scheduled systems, independent of host service types.
pub struct MapContext {
    /// Style evaluated by rendering and tile request systems.
    pub style: Style,
    /// Loaded tile components and shared system resources.
    pub world: World,
    /// Camera, viewport and projection inputs for the current frame.
    pub view_state: ViewState,
    /// GPU state and render graph used to draw this map.
    pub renderer: Renderer,
}

impl MapContext {
    /// The time of the frame being rendered, as the host wrote it; zero until it does.
    pub fn frame_time(&self) -> std::time::Duration {
        crate::render::frame_input::frame_time(&self.world)
    }

    /// The credits of the sources drawn at the camera's zoom, for a host's credits panel; see
    /// [`Style::attributions`].
    pub fn attributions(&self) -> Vec<String> {
        self.style.attributions(self.view_state.zoom().value())
    }

    /// Returns the active zoom and geographic center bounds enforced on [`Self::view_state`].
    pub fn camera_bounds(&self) -> crate::render::view_state::CameraBounds {
        self.view_state.camera_bounds()
    }

    /// Sets or clears the minimum continuous zoom level enforced on the camera.
    pub fn set_min_zoom(&mut self, min_zoom: Option<f64>) {
        self.view_state.set_min_zoom(min_zoom);
        crate::render::frame_signals::mark_dirty(&mut self.world);
    }

    /// Sets or clears the maximum continuous zoom level enforced on the camera.
    pub fn set_max_zoom(&mut self, max_zoom: Option<f64>) {
        self.view_state.set_max_zoom(max_zoom);
        crate::render::frame_signals::mark_dirty(&mut self.world);
    }

    /// Computes the center and maximum zoom that frame `bounds` within `padding`.
    pub fn camera_for_bounds(
        &self,
        bounds: crate::render::view_state::LatLonBounds,
        padding: crate::render::view_state::EdgePadding,
        bearing: Option<cgmath::Deg<f64>>,
        pitch: Option<cgmath::Deg<f64>>,
    ) -> crate::render::view_state::CameraFit {
        self.view_state
            .camera_for_bounds(bounds, padding, bearing, pitch)
    }
}

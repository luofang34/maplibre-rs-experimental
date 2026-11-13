//! The camera a map starts with, jumps made before the renderer is ready, and animated moves.

use super::{CurrentMapContext, Map, MapError};
use crate::{
    context::{FlightId, FlightOutcome},
    coords::{LatLon, WorldCoords, Zoom},
    environment::Environment,
    projection::globe::scale::MERCATOR_LATITUDE_LIMIT,
    render::view_state::{
        CameraBounds, CameraFit, CameraOptions, EdgePadding, FlightCurve, LatLonBounds,
        NavigationError, ViewState,
    },
    style::Style,
    window::{HeadedMapWindow, MapWindow, MapWindowConfig, PhysicalSize},
};

/// The jumps made before the renderer is ready, as one: a later jump's fields replace an
/// earlier one's, and what neither names is kept.
pub(super) fn merged(earlier: Option<CameraOptions>, later: CameraOptions) -> CameraOptions {
    let earlier = earlier.unwrap_or_default();
    CameraOptions {
        center: later.center.or(earlier.center),
        zoom: later.zoom.or(earlier.zoom),
        bearing: later.bearing.or(earlier.bearing),
        pitch: later.pitch.or(earlier.pitch),
        roll: later.roll.or(earlier.roll),
        elevation: later.elevation.or(earlier.elevation),
    }
}

/// The view a map starts rendering with: the one retained across a reset, or the style's
/// initial camera, moved by the jump made while the renderer was not ready.
pub(super) fn starting_view(
    style: &Style,
    retained: Option<&ViewState>,
    window_size: PhysicalSize,
    max_pitch: cgmath::Deg<f64>,
    camera_bounds: CameraBounds,
    jump: Option<CameraOptions>,
) -> Result<ViewState, NavigationError> {
    let mut view = match retained {
        Some(view) => {
            let mut view = view.clone();
            view.clear_external_view();
            view.set_max_pitch(max_pitch);
            view.set_camera_bounds(camera_bounds);
            view
        }
        None => {
            let mut view = initial_view_state(style, window_size, max_pitch);
            view.set_camera_bounds(camera_bounds);
            view
        }
    };
    if let Some(jump) = jump {
        view.jump_to(jump)?;
    }
    Ok(view)
}

/// Returns `style.center` as [`LatLon`] with latitude clamped to [`MERCATOR_LATITUDE_LIMIT`].
pub(crate) fn clamped_style_center(style: &Style) -> LatLon {
    let [lon, lat] = style.center.unwrap_or_default();
    LatLon::new(
        lat.clamp(-MERCATOR_LATITUDE_LIMIT, MERCATOR_LATITUDE_LIMIT),
        lon,
    )
}

fn initial_view_state(
    style: &Style,
    window_size: PhysicalSize,
    max_pitch: cgmath::Deg<f64>,
) -> ViewState {
    let zoom = style.zoom.map(Zoom::new).unwrap_or_default();
    let mut view = ViewState::new(
        window_size,
        WorldCoords::from_lat_lon(clamped_style_center(style), zoom),
        zoom,
        cgmath::Deg(style.pitch.unwrap_or_default()),
        cgmath::Rad(0.6435011087932844),
    );
    view.set_max_pitch(max_pitch);
    view.camera_mut()
        .set_pitch(cgmath::Deg(style.pitch.unwrap_or_default()));
    view.camera_mut()
        .set_bearing(cgmath::Deg(style.bearing.unwrap_or_default()));
    view.camera_mut()
        .set_roll(cgmath::Deg(style.roll.unwrap_or_default()));
    view.take_style_center(style);
    view
}

impl<E: Environment> Map<E>
where
    <<E as Environment>::MapWindowConfig as MapWindowConfig>::MapWindow: HeadedMapWindow,
{
    /// Moves the camera to `options` along `curve`, as
    /// [`crate::context::MapContext::fly_to`]. Frames advance a move, so it needs an
    /// initialized renderer.
    pub fn fly_to(
        &mut self,
        options: CameraOptions,
        curve: FlightCurve,
    ) -> Result<FlightId, MapError> {
        Ok(self.context_mut()?.fly_to(options, curve))
    }

    /// How the move `id` stands, as [`crate::context::MapContext::flight_outcome`]. A map
    /// whose renderer is not ready keeps no moves, so every move reads as
    /// [`FlightOutcome::Superseded`].
    pub fn flight_outcome(&self, id: FlightId) -> FlightOutcome {
        self.context().map_or(FlightOutcome::Superseded, |context| {
            context.flight_outcome(id)
        })
    }

    /// Stops the move under way where the camera is, as
    /// [`crate::context::MapContext::stop_flight`].
    pub fn stop_flight(&mut self) {
        if let Ok(context) = self.context_mut() {
            context.stop_flight();
        }
    }

    /// Returns the active zoom and geographic center bounds enforced on the camera.
    pub fn camera_bounds(&self) -> CameraBounds {
        match &self.map_context {
            CurrentMapContext::Ready(context) => context.camera_bounds(),
            CurrentMapContext::Pending(_) => self.camera_bounds,
        }
    }

    /// Replaces the zoom and geographic center bounds, before or after renderer initialization.
    pub fn set_camera_bounds(&mut self, bounds: CameraBounds) {
        self.camera_bounds = bounds;
        if let CurrentMapContext::Ready(context) = &mut self.map_context {
            context.set_camera_bounds(bounds);
        }
    }

    /// Sets or clears the minimum continuous zoom level enforced on the camera.
    pub fn set_min_zoom(&mut self, min_zoom: Option<f64>) {
        self.camera_bounds.min_zoom = min_zoom.filter(|v| v.is_finite());
        if let CurrentMapContext::Ready(context) = &mut self.map_context {
            context.set_min_zoom(min_zoom);
        }
    }

    /// Sets or clears the maximum continuous zoom level enforced on the camera.
    pub fn set_max_zoom(&mut self, max_zoom: Option<f64>) {
        self.camera_bounds.max_zoom = max_zoom.filter(|v| v.is_finite());
        if let CurrentMapContext::Ready(context) = &mut self.map_context {
            context.set_max_zoom(max_zoom);
        }
    }

    /// Sets or clears the geographic bounding box restricting the camera's center.
    pub fn set_max_bounds(&mut self, max_bounds: Option<LatLonBounds>) {
        self.camera_bounds.max_bounds = max_bounds;
        if let CurrentMapContext::Ready(context) = &mut self.map_context {
            context.set_max_bounds(max_bounds);
        }
    }

    /// Computes the center and maximum zoom that frame `bounds` within `padding`, before or after
    /// renderer initialization.
    pub fn camera_for_bounds(
        &self,
        bounds: LatLonBounds,
        padding: EdgePadding,
        bearing: Option<cgmath::Deg<f64>>,
        pitch: Option<cgmath::Deg<f64>>,
    ) -> Result<CameraFit, MapError> {
        match &self.map_context {
            CurrentMapContext::Ready(context) => {
                Ok(context.camera_for_bounds(bounds, padding, bearing, pitch))
            }
            CurrentMapContext::Pending(pending) => {
                let window_size = self.window.size();
                let mut temp_view = starting_view(
                    &pending.style,
                    pending.view_state.as_ref(),
                    window_size,
                    self.max_pitch,
                    self.camera_bounds,
                    pending.jump,
                )?;
                temp_view.resize(window_size.to_logical(self.window.scale_factor()));
                Ok(temp_view.camera_for_bounds(bounds, padding, bearing, pitch))
            }
        }
    }

    /// Moves the camera immediately to frame `bounds` within `padding`, before or after renderer
    /// initialization.
    pub fn fit_bounds(
        &mut self,
        bounds: LatLonBounds,
        padding: EdgePadding,
    ) -> Result<CameraFit, MapError> {
        let window_size = self.window.size();
        let scale_factor = self.window.scale_factor();
        let max_pitch = self.max_pitch;
        let camera_bounds = self.camera_bounds;
        match &mut self.map_context {
            CurrentMapContext::Ready(context) => Ok(context.fit_bounds(bounds, padding)?),
            CurrentMapContext::Pending(pending) => {
                let mut temp_view = starting_view(
                    &pending.style,
                    pending.view_state.as_ref(),
                    window_size,
                    max_pitch,
                    camera_bounds,
                    pending.jump,
                )?;
                temp_view.resize(window_size.to_logical(scale_factor));
                let fit = temp_view.camera_for_bounds(bounds, padding, None, None);
                pending.jump = Some(merged(pending.jump, fit.into()));
                Ok(fit)
            }
        }
    }
}

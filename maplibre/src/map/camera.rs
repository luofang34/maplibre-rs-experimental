//! The camera a map starts with, jumps made before the renderer is ready, and animated moves.

use super::{Map, MapError};
use crate::{
    context::{FlightId, FlightOutcome},
    coords::{LatLon, WorldCoords, Zoom},
    environment::Environment,
    projection::globe::scale::MERCATOR_LATITUDE_LIMIT,
    render::view_state::{CameraOptions, FlightCurve, NavigationError, ViewState},
    style::Style,
    window::{HeadedMapWindow, MapWindowConfig, PhysicalSize},
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
    jump: Option<CameraOptions>,
) -> Result<ViewState, NavigationError> {
    let mut view = match retained {
        Some(view) => {
            let mut view = view.clone();
            view.clear_external_view();
            view.set_max_pitch(max_pitch);
            view
        }
        None => initial_view_state(style, window_size, max_pitch),
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
}

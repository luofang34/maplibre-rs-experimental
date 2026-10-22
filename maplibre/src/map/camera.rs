//! The camera a map starts with, and jumps made before the renderer is ready.

use crate::{
    coords::{LatLon, WorldCoords, Zoom},
    render::view_state::{CameraOptions, ViewState},
    style::Style,
    window::PhysicalSize,
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
) -> ViewState {
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
        view.jump_to(jump);
    }
    view
}

fn initial_view_state(
    style: &Style,
    window_size: PhysicalSize,
    max_pitch: cgmath::Deg<f64>,
) -> ViewState {
    let center = style.center.unwrap_or_default();
    let zoom = style.zoom.map(Zoom::new).unwrap_or_default();
    let mut view = ViewState::new(
        window_size,
        WorldCoords::from_lat_lon(LatLon::new(center[1], center[0]), zoom),
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

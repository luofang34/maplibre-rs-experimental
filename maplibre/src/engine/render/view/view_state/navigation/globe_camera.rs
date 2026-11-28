use cgmath::Point2;

use super::super::ViewState;
use crate::{
    coords::{LatLon, TILE_SIZE},
    projection::globe::camera::{GlobeCameraOptions, GlobeCameraState},
    render::error::ProjectionStateError,
};

impl ViewState {
    /// Constructs the vertical-perspective camera matching this view state.
    pub(crate) fn globe_camera(&self) -> Result<GlobeCameraState, ProjectionStateError> {
        let external_eye = self.external_globe_eye();
        // A free-globe camera is drawn from its pose; a host's eye replaces either.
        let pose = self.pose_view().filter(|_| external_eye.is_none());
        let world_size = TILE_SIZE
            * 2.0_f64.powf(
                pose.as_ref()
                    .map_or(self.zoom().value(), |pose| pose.style_zoom),
            );
        let camera_position = self.camera().position();
        let center = pose.as_ref().map_or_else(
            || mercator_world_to_lat_lon(camera_position.x, camera_position.y, world_size),
            |pose| pose.center,
        );
        let angle = |free: Option<f64>, flat: f64| free.unwrap_or(flat);
        let options = GlobeCameraOptions {
            width: self.width(),
            height: self.height(),
            field_of_view_degrees: external_eye.map_or_else(
                || self.field_of_view().0.to_degrees(),
                |eye| eye.frustum.vertical_field_of_view().0.to_degrees(),
            ),
            center,
            world_size,
            bearing_degrees: angle(
                pose.as_ref().map(|pose| pose.bearing_degrees),
                self.camera().get_bearing().0.to_degrees(),
            ),
            pitch_degrees: angle(
                pose.as_ref().map(|pose| pose.pitch_degrees),
                self.camera().get_pitch().0.to_degrees(),
            ),
            roll_degrees: angle(
                pose.as_ref().map(|pose| pose.roll_degrees),
                self.camera().get_roll().0.to_degrees(),
            ),
            center_offset: external_eye
                .map_or_else(|| self.center_offset(), |_| Point2::new(0.0, 0.0)),
            body: self.body(),
            target_elevation_meters: if external_eye.is_none()
                && (pose.is_some() || self.globe_orbits_center())
            {
                self.center_elevation()
            } else {
                0.0
            },
            radius_pixels: pose.as_ref().map(|pose| pose.radius_pixels),
        };
        match external_eye {
            Some(eye) => GlobeCameraState::from_external_eye(options, eye),
            None => GlobeCameraState::new(options),
        }
        .map_err(|source| ProjectionStateError::GlobeCamera { source })
    }
}

/// Constructs the vertical-perspective camera matching the current map view.
pub fn globe_camera_for_view(
    view_state: &ViewState,
) -> Result<GlobeCameraState, ProjectionStateError> {
    view_state.globe_camera()
}

pub(crate) fn mercator_world_to_lat_lon(x: f64, y: f64, world_size: f64) -> LatLon {
    let longitude = x / world_size * 360.0 - 180.0;
    let latitude = (std::f64::consts::PI * (1.0 - 2.0 * y / world_size))
        .sinh()
        .atan()
        .to_degrees();
    LatLon::new(latitude, longitude)
}

//! Camera settings a host makes on the offscreen map, beyond what its style starts it with.

use super::HeadlessMap;

impl HeadlessMap {
    /// Puts the camera's center at `meters` above sea level and holds it there, so terrain under
    /// the center does not move it, as GL JS does once `centerClampedToGround` is off.
    pub fn pin_center_elevation(&mut self, meters: f64) {
        let view_state = &mut self.map_context.view_state;
        view_state.set_center_clamped_to_ground(false);
        view_state.set_center_altitude(meters);
    }

    /// Switches how the camera navigates the globe, for the style's projection.
    pub fn set_navigation_mode(
        &mut self,
        mode: crate::render::view_state::NavigationMode,
    ) -> Result<(), crate::render::view_state::NavigationError> {
        self.map_context.set_navigation_mode(mode)
    }

    /// Moves the map at once, as GL JS `jumpTo`, standing a center held by terrain on the
    /// loaded ground or at sea level, as [`crate::context::MapContext::jump_to`] does.
    pub fn jump_to(
        &mut self,
        options: crate::render::view_state::CameraOptions,
    ) -> Result<(), crate::render::view_state::NavigationError> {
        self.map_context.jump_to(options)
    }

    /// Moves the camera along `curve`, as [`crate::context::MapContext::fly_to`] does; read how
    /// the move stands through [`Self::context`].
    pub fn fly_to(
        &mut self,
        options: crate::render::view_state::CameraOptions,
        curve: crate::render::view_state::FlightCurve,
    ) -> crate::context::FlightId {
        self.map_context.fly_to(options, curve)
    }

    /// Stops the camera move under way where the camera is.
    pub fn stop_flight(&mut self) {
        self.map_context.stop_flight();
    }

    /// Restores a stored free-globe pose, for the style's projection.
    pub fn restore_globe_pose(
        &mut self,
        pose: crate::render::view_state::GlobePose,
    ) -> Result<(), crate::render::view_state::NavigationError> {
        self.map_context.restore_globe_pose(pose)
    }

    /// Sets the full vertical field of view, as the GL JS `verticalFieldOfView` map option.
    pub fn set_vertical_field_of_view(&mut self, field_of_view: cgmath::Deg<f64>) {
        self.map_context
            .view_state
            .set_field_of_view(cgmath::Rad::from(field_of_view));
    }

    /// Asks for an opaque environment: on a full globe without atmosphere the style's sky
    /// then fills everything the globe does not cover, as it does for a
    /// [`crate::headless::MultiViewFrame`] that asks the same.
    pub fn set_opaque_environment(&mut self, opaque: bool) {
        self.map_context.view_state.set_opaque_environment(opaque);
        crate::render::frame_signals::mark_dirty(&mut self.map_context.world);
    }

    /// Drags a free-globe camera so the ground under `from` moves under `to`, as
    /// [`crate::render::view_state::ViewState::drag_free_globe`] does, and marks the map for
    /// redraw when it moved. Returns whether it moved.
    pub fn drag_globe(&mut self, from: cgmath::Point2<f64>, to: cgmath::Point2<f64>) -> bool {
        let moved = self.map_context.view_state.drag_free_globe(from, to);
        if moved {
            crate::render::frame_signals::mark_dirty(&mut self.map_context.world);
        }
        moved
    }

    /// Zooms a free-globe camera by `zoom_delta` about the ground under `pixel`, as
    /// [`crate::render::view_state::ViewState::zoom_free_globe`] does, and marks the map for
    /// redraw when it moved. Returns whether it moved.
    pub fn zoom_globe(&mut self, pixel: cgmath::Point2<f64>, zoom_delta: f64) -> bool {
        let moved = self
            .map_context
            .view_state
            .zoom_free_globe(pixel, zoom_delta);
        if moved {
            crate::render::frame_signals::mark_dirty(&mut self.map_context.world);
        }
        moved
    }

    /// The sea-level location under a viewport pixel of the globe camera, or `None` where the
    /// pixel shows space beyond the limb or the view has no globe camera.
    pub fn screen_to_globe_lat_lon(
        &self,
        pixel: cgmath::Point2<f64>,
    ) -> Option<crate::coords::LatLon> {
        let camera =
            crate::render::projection::globe_camera_for_view(&self.map_context.view_state).ok()?;
        camera
            .is_point_on_map_surface(pixel)
            .then(|| camera.screen_point_to_location(pixel))
            .flatten()
    }

    /// Where the globe camera draws `location` raised `elevation_meters`, in viewport pixels,
    /// and whether the planet hides it; `None` when the view has no globe camera.
    pub fn globe_lat_lon_to_screen(
        &self,
        location: crate::coords::LatLon,
        elevation_meters: f64,
    ) -> Option<(cgmath::Point2<f64>, bool)> {
        let camera =
            crate::render::projection::globe_camera_for_view(&self.map_context.view_state).ok()?;
        Some((
            camera.location_to_screen(location, elevation_meters),
            camera.is_location_occluded(location),
        ))
    }
}

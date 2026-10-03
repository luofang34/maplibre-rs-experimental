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
}

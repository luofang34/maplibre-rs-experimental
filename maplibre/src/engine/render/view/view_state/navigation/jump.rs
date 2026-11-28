//! Immediate camera jumps (`jumpTo`) resolved to constrained map coordinates or a free-globe pose.

use super::{NavigationError, NavigationMode};
use crate::{
    coords::LatLon,
    projection::globe::scale::MERCATOR_LATITUDE_LIMIT,
    render::view_state::{CameraOptions, ViewState},
};

impl ViewState {
    /// Moves the map at once, as GL JS `jumpTo`: to the center, zoom and angles given, with
    /// the others kept as they are. A zoom that is not a number is left out, as GL JS
    /// constrains a jump.
    ///
    /// Constrained navigation keeps the center's latitude within the Mercator limit; free-globe
    /// navigation places the pose [`Self::globe_pose_for`] gives. The center rests at the
    /// height [`Self::jump_elevation`] gives, until terrain holding it stands it on the ground
    /// under it, as [`crate::context::MapContext::jump_to`] does at once.
    pub fn jump_to(&mut self, options: CameraOptions) -> Result<(), NavigationError> {
        if self.navigation_mode() == NavigationMode::FreeGlobe {
            return self.set_globe_pose(self.globe_pose_for(options));
        }
        let jump = self.resolve_jump(options);
        let center = LatLon::new(
            jump.center
                .latitude
                .clamp(-MERCATOR_LATITUDE_LIMIT, MERCATOR_LATITUDE_LIMIT),
            jump.center.longitude,
        );
        self.set_center_and_angles(center, jump.zoom, jump.bearing, jump.pitch, jump.roll);
        if let Some(elevation) = self.jump_elevation(options) {
            self.rest_center_at(elevation);
        }
        Ok(())
    }
}

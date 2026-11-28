//! Feature-backed methods on [`MapContext`] touching placement, pixel ratio, and terrain.

use crate::{context::MapContext, window::PhysicalSize};

impl MapContext {
    /// Configures symbol collision cadence and fade duration for subsequent frames.
    pub fn set_placement_config(&mut self, config: crate::sdf::PlacementConfig) {
        self.world.resources.insert(config);
        crate::render::frame_signals::mark_dirty(&mut self.world);
    }

    /// Updates the logical viewport and resizes the physical presentation surface.
    /// `scale_factor` is the number of physical pixels per logical unit.
    /// Logical dimensions remain at least one unit even below the device scale.
    pub fn resize(&mut self, size: PhysicalSize, scale_factor: f64) {
        self.view_state.resize(size.to_logical(scale_factor));
        crate::sdf::provided::set_pixel_ratio(&mut self.world, scale_factor);
        self.renderer.resize_surface(size);
        // A new surface holds nothing, even when the logical size and so the camera are unchanged.
        crate::render::frame_signals::mark_dirty(&mut self.world);
    }

    /// Switches how the camera navigates the globe, for the style's projection: free-globe
    /// navigation needs the pure `vertical-perspective` projection. A center held by
    /// terrain stands on the loaded ground under it at once, as
    /// [`crate::terrain::elevation::ground_center`] stands it.
    pub fn set_navigation_mode(
        &mut self,
        mode: crate::render::view_state::NavigationMode,
    ) -> Result<(), crate::render::view_state::NavigationError> {
        let projection = self
            .style
            .projection
            .as_ref()
            .map_or_else(Default::default, |projection| {
                projection.projection_type.clone()
            });
        self.view_state.set_navigation_mode(mode, &projection)?;
        crate::terrain::elevation::ground_center(&mut self.view_state, &self.world);
        Ok(())
    }

    /// Moves the map at once, as GL JS `jumpTo`, in either navigation mode, as
    /// [`ViewState::jump_to`](crate::render::view_state::ViewState::jump_to) moves it. A center
    /// held by terrain stands on the loaded ground under it at once, or at sea level where none
    /// has loaded, never on the ground it left.
    pub fn jump_to(
        &mut self,
        options: crate::render::view_state::CameraOptions,
    ) -> Result<(), crate::render::view_state::NavigationError> {
        self.view_state.jump_to(options)?;
        crate::terrain::interaction::ground_and_clear_terrain(
            &self.style,
            &mut self.view_state,
            &self.world,
        );
        Ok(())
    }

    /// Replaces the zoom and geographic center bounds and clamps the camera if needed.
    pub fn set_camera_bounds(&mut self, bounds: crate::render::view_state::CameraBounds) {
        self.view_state.set_camera_bounds(bounds);
        crate::terrain::interaction::ground_and_clear_terrain(
            &self.style,
            &mut self.view_state,
            &self.world,
        );
        crate::render::frame_signals::mark_dirty(&mut self.world);
    }

    /// Sets or clears the geographic bounding box restricting the camera's center.
    pub fn set_max_bounds(&mut self, max_bounds: Option<crate::render::view_state::LatLonBounds>) {
        self.view_state.set_max_bounds(max_bounds);
        crate::terrain::interaction::ground_and_clear_terrain(
            &self.style,
            &mut self.view_state,
            &self.world,
        );
        crate::render::frame_signals::mark_dirty(&mut self.world);
    }

    /// Moves the camera immediately to frame `bounds` within `padding`.
    pub fn fit_bounds(
        &mut self,
        bounds: crate::render::view_state::LatLonBounds,
        padding: crate::render::view_state::EdgePadding,
    ) -> Result<crate::render::view_state::CameraFit, crate::render::view_state::NavigationError>
    {
        let fit = self.view_state.fit_bounds(bounds, padding)?;
        crate::terrain::interaction::ground_and_clear_terrain(
            &self.style,
            &mut self.view_state,
            &self.world,
        );
        crate::render::frame_signals::mark_dirty(&mut self.world);
        Ok(fit)
    }

    /// Restores a stored free-globe pose, for the style's projection: it must be the pure
    /// `vertical-perspective` projection. A center held by terrain stands on the loaded
    /// ground under the pose's target at once, rather than at the pose's own target height, as
    /// [`crate::terrain::elevation::ground_center`] stands it.
    pub fn restore_globe_pose(
        &mut self,
        pose: crate::render::view_state::GlobePose,
    ) -> Result<(), crate::render::view_state::NavigationError> {
        let projection = self
            .style
            .projection
            .as_ref()
            .map_or_else(Default::default, |projection| {
                projection.projection_type.clone()
            });
        self.view_state.restore_globe_pose(pose, &projection)?;
        crate::terrain::elevation::ground_center(&mut self.view_state, &self.world);
        Ok(())
    }
}

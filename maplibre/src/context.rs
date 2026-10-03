//! Style, tile storage, view and renderer state for a map frame.

#![deny(missing_docs)]

use crate::{
    render::{view_state::ViewState, Renderer},
    style::Style,
    tcs::world::World,
    window::PhysicalSize,
};

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
    /// Updates the logical viewport and resizes the physical presentation surface.
    /// `scale_factor` is the number of physical pixels per logical unit.
    /// Logical dimensions remain at least one unit even below the device scale.
    pub fn resize(&mut self, size: PhysicalSize, scale_factor: f64) {
        self.view_state.resize(size.to_logical(scale_factor));
        self.renderer.resize_surface(size);
        // A new surface holds nothing, even when the logical size and so the camera are unchanged.
        crate::render::frame_signals::mark_dirty(&mut self.world);
    }

    /// Switches how the camera navigates the globe, for the style's projection: free-globe
    /// navigation needs the pure `vertical-perspective` projection.
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
        self.view_state.set_navigation_mode(mode, &projection)
    }

    /// Restores a stored free-globe pose, for the style's projection: it must be the pure
    /// `vertical-perspective` projection.
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
        self.view_state.restore_globe_pose(pose, &projection)
    }
}

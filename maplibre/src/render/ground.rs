//! A surface, such as terrain, that the main pass draws between the style's layers.

use crate::{render::tracked_pass::TrackedRenderPass, tcs::world::World};

/// Records a ground surface's draws into the main pass.
pub type DrawGround = for<'w> fn(&mut TrackedRenderPass<'w>, &'w World);

/// The ground surface a frame draws.
#[derive(Clone, Copy)]
pub struct GroundSurface {
    /// The surface draws before the first layer whose index is above this one, so layers
    /// beneath it, such as the background, stay beneath it.
    pub draw_after_layer_index: u32,
    /// Records the surface.
    pub draw: DrawGround,
}

/// The ground surface of the frame, set each frame by the plugin that owns the surface.
///
/// While a frame has a ground surface, every layer draws in the main pass, so the surface's
/// depth covers the layers it hides.
#[derive(Clone, Copy, Default)]
pub struct FrameGround(pub Option<GroundSurface>);

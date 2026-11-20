//! Extruded-polygon (`fill-extrusion`) pipeline binding and draw commands.

use crate::{
    render::{
        eventually::{Eventually, Eventually::Initialized},
        projection::ProjectionGpuResources,
        render_phase::{LayerItem, PhaseItem, RenderCommand, RenderCommandResult},
    },
    tcs::world::World,
    vector::{render_commands::DrawVectorTile, ExtrusionPipeline},
};

/// Binds one pass of the extrusion pipeline (selecting the 8-byte compact pipeline when the entry
/// uses uniform height/base encoding) and the item's view or flat projection.
pub struct SetExtrusionPipeline<const PASS: u8>;

/// The pass an extrusion draw belongs to.
pub mod extrusion_pass {
    /// Depth only.
    pub const DEPTH: u8 = 0;
    /// Colour once per pixel.
    pub const COLOR: u8 = 1;
    /// Colour from an image, once per pixel.
    pub const PATTERN: u8 = 3;
    /// Back-facing shell of a translucent 3D volume.
    pub const TRANSLUCENT_BACK: u8 = 4;
    /// Front-facing shell of a translucent 3D volume.
    pub const TRANSLUCENT_FRONT: u8 = 5;
}

impl<const PASS: u8> RenderCommand<LayerItem> for SetExtrusionPipeline<PASS> {
    fn render<'w>(
        world: &'w World,
        item: &LayerItem,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some((Initialized(pipeline), Initialized(projection_resources))) =
            world.resources.query::<(
                &Eventually<ExtrusionPipeline>,
                &Eventually<ProjectionGpuResources>,
            )>()
        else {
            return RenderCommandResult::Failure;
        };

        let Some(mode) =
            pipeline.for_globe(projection_resources.is_globe_active(item.projection_binding()))
        else {
            return RenderCommandResult::Failure;
        };
        let entry = super::resolve_vector_entry(world, item);
        let compact = PASS != extrusion_pass::PATTERN
            && entry.is_some_and(crate::vector::resource::IndexEntry::is_compact_extrusion);
        let uniform =
            entry.is_some_and(crate::vector::resource::IndexEntry::is_uniform_feature_metadata);
        let selected = if compact {
            match PASS {
                extrusion_pass::DEPTH => mode.compact_depth.as_ref(),
                extrusion_pass::COLOR => mode.compact_color.as_ref(),
                extrusion_pass::TRANSLUCENT_BACK => mode.compact_translucent_back.as_ref(),
                extrusion_pass::TRANSLUCENT_FRONT => mode.compact_translucent_front.as_ref(),
                _ => None,
            }
        } else {
            match PASS {
                extrusion_pass::DEPTH => mode.depth.as_ref(),
                extrusion_pass::COLOR => mode.color.as_ref().map(|p| p.select(uniform)),
                extrusion_pass::PATTERN => mode.pattern.as_ref().map(|p| p.select(uniform)),
                extrusion_pass::TRANSLUCENT_BACK => {
                    mode.translucent_back.as_ref().map(|p| p.select(uniform))
                }
                extrusion_pass::TRANSLUCENT_FRONT => {
                    mode.translucent_front.as_ref().map(|p| p.select(uniform))
                }
                _ => None,
            }
        };
        let Some(selected) = selected else {
            return RenderCommandResult::Failure;
        };
        pass.set_pipeline(selected);
        pass.set_bind_group(
            0,
            projection_resources.bind_group_for(item.projection_binding()),
            &[],
        );
        RenderCommandResult::Success
    }
}

/// Clears bit 7 (`STENCIL_MARK`) across the viewport in a single 3-vertex fullscreen triangle.
pub struct DrawExtrusionStencilClear;
impl RenderCommand<LayerItem> for DrawExtrusionStencilClear {
    fn render<'w>(
        world: &'w World,
        _item: &LayerItem,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some(Initialized(pipeline)) = world.resources.get::<Eventually<ExtrusionPipeline>>()
        else {
            return RenderCommandResult::Failure;
        };
        let Some(clear_mark) = pipeline.clear_mark.as_ref() else {
            return RenderCommandResult::Failure;
        };
        pass.set_pipeline(clear_mark);
        pass.draw(0..3, 0..1);
        RenderCommandResult::Success
    }
}

/// Draws an extruded bucket into depth only.
pub type DrawExtrusionDepth = (
    SetExtrusionPipeline<{ extrusion_pass::DEPTH }>,
    DrawVectorTile,
);
/// Draws an extruded bucket where its depth pass left the nearest surface, once per pixel.
pub type DrawExtrusionColor = (
    SetExtrusionPipeline<{ extrusion_pass::COLOR }>,
    DrawVectorTile,
);
/// Draws an extruded bucket from an image, once per pixel.
pub type DrawExtrusionPatternColor = (
    SetExtrusionPipeline<{ extrusion_pass::PATTERN }>,
    super::SetPatternBindGroup,
    DrawVectorTile<true>,
);
/// Draws the back-facing shell of a translucent 3D volume bucket.
pub type DrawExtrusionTranslucentBack = (
    SetExtrusionPipeline<{ extrusion_pass::TRANSLUCENT_BACK }>,
    DrawVectorTile,
);
/// Draws the front-facing shell of a translucent 3D volume bucket.
pub type DrawExtrusionTranslucentFront = (
    SetExtrusionPipeline<{ extrusion_pass::TRANSLUCENT_FRONT }>,
    DrawVectorTile,
);

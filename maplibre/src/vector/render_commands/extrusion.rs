//! Extruded-polygon (`fill-extrusion`) pipeline binding and draw commands.

use crate::{
    render::{
        eventually::{Eventually, Eventually::Initialized},
        projection::ProjectionGpuResources,
        render_phase::{LayerItem, PhaseItem, RenderCommand, RenderCommandResult},
        tile_view_pattern::WgpuTileViewPattern,
    },
    tcs::world::World,
    vector::{render_commands::DrawVectorTile, ExtrusionPipeline, VectorBufferPool},
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

fn select_extrusion_pipeline<'a>(
    mode: &'a crate::vector::ExtrusionModePipelines,
    pass_kind: u8,
    entry: &crate::vector::resource::IndexEntry,
) -> Option<&'a wgpu::RenderPipeline> {
    let compact = pass_kind != extrusion_pass::PATTERN && entry.is_compact_extrusion();
    let uniform = entry.is_uniform_feature_metadata();
    if compact {
        match pass_kind {
            extrusion_pass::DEPTH => mode.compact_depth.as_ref(),
            extrusion_pass::COLOR => mode.compact_color.as_ref(),
            extrusion_pass::TRANSLUCENT_BACK => mode.compact_translucent_back.as_ref(),
            extrusion_pass::TRANSLUCENT_FRONT => mode.compact_translucent_front.as_ref(),
            _ => None,
        }
    } else {
        match pass_kind {
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
    }
}

/// Fused command binding the extrusion pipeline and drawing the extruded bucket in one query and
/// entry lookup.
pub struct DrawExtrusion<const PASS: u8, const RUNS: bool = false>;
impl<const PASS: u8, const RUNS: bool> RenderCommand<LayerItem> for DrawExtrusion<PASS, RUNS> {
    fn render<'w>(
        world: &'w World,
        item: &LayerItem,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some((
            Initialized(pipeline),
            Initialized(projection_resources),
            Initialized(buffer_pool),
            Initialized(tile_view_pattern),
        )) = world.resources.query::<(
            &Eventually<ExtrusionPipeline>,
            &Eventually<ProjectionGpuResources>,
            &Eventually<VectorBufferPool>,
            &Eventually<WgpuTileViewPattern>,
        )>()
        else {
            return RenderCommandResult::Failure;
        };
        let Some(mode) =
            pipeline.for_globe(projection_resources.is_globe_active(item.projection_binding()))
        else {
            return RenderCommandResult::Failure;
        };
        let Some(entry) =
            buffer_pool
                .index()
                .resolve_entry(item.tile.coords, item.style_layer, item.entry_slot)
        else {
            return RenderCommandResult::Failure;
        };
        if entry.indices_buffer_range().is_empty() {
            tracing::error!("Tried to draw a vector tile without any vertices");
            return RenderCommandResult::Failure;
        }
        let Some(selected) = select_extrusion_pipeline(mode, PASS, entry) else {
            return RenderCommandResult::Failure;
        };
        if PASS == extrusion_pass::PATTERN {
            let Some(patterns) = world
                .resources
                .get::<crate::vector::pattern::PatternResources>()
            else {
                return RenderCommandResult::Failure;
            };
            match patterns.binding(item.style_layer) {
                Some(binding) => pass.set_bind_group(1, binding, &[]),
                None if patterns.is_per_feature(item.style_layer) => {}
                None => return RenderCommandResult::Failure,
            }
        }
        pass.set_pipeline(selected);
        pass.set_bind_group(
            0,
            projection_resources.bind_group_for(item.projection_binding()),
            &[],
        );
        DrawVectorTile::<RUNS>::draw_prepared(
            world,
            buffer_pool,
            tile_view_pattern,
            entry,
            item,
            pass,
        )
    }
}

/// Draws an extruded bucket into depth only.
pub type DrawExtrusionDepth = DrawExtrusion<{ extrusion_pass::DEPTH }>;
/// Draws an extruded bucket where its depth pass left the nearest surface, once per pixel.
pub type DrawExtrusionColor = DrawExtrusion<{ extrusion_pass::COLOR }>;
/// Draws an extruded bucket from an image, once per pixel.
pub type DrawExtrusionPatternColor = DrawExtrusion<{ extrusion_pass::PATTERN }, true>;
/// Draws the back-facing shell of a translucent 3D volume bucket.
pub type DrawExtrusionTranslucentBack = DrawExtrusion<{ extrusion_pass::TRANSLUCENT_BACK }>;
/// Draws the front-facing shell of a translucent 3D volume bucket.
pub type DrawExtrusionTranslucentFront = DrawExtrusion<{ extrusion_pass::TRANSLUCENT_FRONT }>;

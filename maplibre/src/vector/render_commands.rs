//! Fill, line and circle commands that skip draws until GPU geometry and paint are ready.
use crate::{
    render::{
        eventually::{Eventually, Eventually::Initialized},
        projection::ProjectionGpuResources,
        render_phase::{LayerItem, PhaseItem, RenderCommand, RenderCommandResult},
        tile_view_pattern::WgpuTileViewPattern,
        INDEX_FORMAT,
    },
    tcs::world::World,
    vector::{CirclePipeline, ExtrusionPipeline, LinePipeline, VectorBufferPool, VectorPipeline},
};

/// Binds the polygon-fill pipeline and the item's view or flat projection.
pub struct SetVectorTilePipeline;
impl<P: PhaseItem> RenderCommand<P> for SetVectorTilePipeline {
    fn render<'w>(
        world: &'w World,
        item: &P,
        pass: &mut wgpu::RenderPass<'w>,
    ) -> RenderCommandResult {
        let Some((Initialized(pipeline), Initialized(projection_resources))) =
            world.resources.query::<(
                &Eventually<VectorPipeline>,
                &Eventually<ProjectionGpuResources>,
            )>()
        else {
            return RenderCommandResult::Failure;
        };

        pass.set_pipeline(pipeline);
        pass.set_bind_group(
            0,
            projection_resources.bind_group_for(item.projection_binding()),
            &[],
        );
        RenderCommandResult::Success
    }
}

/// Draws the tile's matching style-layer geometry with tile, layer and feature metadata.
pub struct DrawVectorTile;
impl RenderCommand<LayerItem> for DrawVectorTile {
    fn render<'w>(
        world: &'w World,
        item: &LayerItem,
        pass: &mut wgpu::RenderPass<'w>,
    ) -> RenderCommandResult {
        let Some((Initialized(buffer_pool), Initialized(tile_view_pattern))) =
            world.resources.query::<(
                &Eventually<VectorBufferPool>,
                &Eventually<WgpuTileViewPattern>,
            )>()
        else {
            return RenderCommandResult::Failure;
        };

        let Some(vector_layers) = buffer_pool.index().get_layers(item.tile.coords) else {
            return RenderCommandResult::Failure;
        };

        let Some(entry) = vector_layers
            .iter()
            .find(|entry| entry.style_layer.id == item.style_layer)
        else {
            return RenderCommandResult::Failure;
        };

        let source_shape = &item.source_shape;

        let reference = source_shape.coords().stencil_reference_value_3d() as u32;

        tracing::trace!(
            "Drawing layer {:?} at {}",
            entry.style_layer.source_layer,
            entry.coords
        );

        let index_range = entry.indices_buffer_range();

        if index_range.is_empty() {
            tracing::error!("Tried to draw a vector tile without any vertices");
            return RenderCommandResult::Failure;
        }

        pass.set_stencil_reference(reference);

        pass.set_index_buffer(buffer_pool.indices().slice(index_range), INDEX_FORMAT);
        pass.set_vertex_buffer(
            0,
            buffer_pool.vertices().slice(entry.vertices_buffer_range()),
        );
        let Some(tile_view_pattern_buffer) = source_shape.buffer_range() else {
            return RenderCommandResult::Failure;
        };
        pass.set_vertex_buffer(
            1,
            tile_view_pattern.buffer().slice(tile_view_pattern_buffer),
        );
        pass.set_vertex_buffer(
            2,
            buffer_pool
                .metadata()
                .slice(entry.layer_metadata_buffer_range()),
        );
        pass.set_vertex_buffer(
            3,
            buffer_pool
                .feature_metadata()
                .slice(entry.feature_metadata_buffer_range()),
        );
        pass.draw_indexed(entry.indices_range(), 0, 0..1);

        RenderCommandResult::Success
    }
}

/// Binds the line pipeline and dash atlas; elevated structures use depth-aware drawing on terrain.
pub struct SetLineTilePipeline;
impl RenderCommand<LayerItem> for SetLineTilePipeline {
    fn render<'w>(
        world: &'w World,
        item: &LayerItem,
        pass: &mut wgpu::RenderPass<'w>,
    ) -> RenderCommandResult {
        let Some((Initialized(pipeline), Initialized(projection_resources))) =
            world.resources.query::<(
                &Eventually<LinePipeline>,
                &Eventually<ProjectionGpuResources>,
            )>()
        else {
            return RenderCommandResult::Failure;
        };

        let spatial = world
            .resources
            .get::<crate::terrain::TerrainFrame>()
            .is_some_and(|frame| frame.active)
            && world
                .resources
                .get::<Eventually<VectorBufferPool>>()
                .and_then(|pool| match pool {
                    Initialized(pool) => pool.index().get_layers(item.tile.coords),
                    _ => None,
                })
                .and_then(|layers| {
                    layers
                        .iter()
                        .find(|entry| entry.style_layer.id == item.style_layer)
                })
                .is_some_and(|entry| super::structures::kind(&entry.style_layer).is_some());
        pass.set_pipeline(if spatial { &pipeline.1 } else { &pipeline.0 });
        pass.set_bind_group(
            0,
            projection_resources.bind_group_for(item.projection_binding()),
            &[],
        );
        let Some(dashes) = world.resources.get::<super::line_dash::LineDashResources>() else {
            return RenderCommandResult::Failure;
        };
        pass.set_bind_group(1, dashes.binding(&item.style_layer), &[]);
        RenderCommandResult::Success
    }
}

/// Binds the circle pipeline and the item's view or flat projection.
pub struct SetCircleTilePipeline;
impl<P: PhaseItem> RenderCommand<P> for SetCircleTilePipeline {
    fn render<'w>(
        world: &'w World,
        item: &P,
        pass: &mut wgpu::RenderPass<'w>,
    ) -> RenderCommandResult {
        let Some((Initialized(pipeline), Initialized(projection_resources))) =
            world.resources.query::<(
                &Eventually<CirclePipeline>,
                &Eventually<ProjectionGpuResources>,
            )>()
        else {
            return RenderCommandResult::Failure;
        };

        pass.set_pipeline(pipeline);
        pass.set_bind_group(
            0,
            projection_resources.bind_group_for(item.projection_binding()),
            &[],
        );
        RenderCommandResult::Success
    }
}

/// Binds the pattern pipeline and the pattern of the item's layer.
pub struct SetPatternTilePipeline;
impl RenderCommand<LayerItem> for SetPatternTilePipeline {
    fn render<'w>(
        world: &'w World,
        item: &LayerItem,
        pass: &mut wgpu::RenderPass<'w>,
    ) -> RenderCommandResult {
        let (Some(patterns), Some(Initialized(projection_resources))) = (
            world.resources.get::<super::pattern::PatternResources>(),
            world.resources.get::<Eventually<ProjectionGpuResources>>(),
        ) else {
            return RenderCommandResult::Failure;
        };
        let Some(binding) = patterns.binding(&item.style_layer) else {
            return RenderCommandResult::Failure;
        };
        pass.set_pipeline(patterns.pipeline());
        pass.set_bind_group(
            0,
            projection_resources.bind_group_for(item.projection_binding()),
            &[],
        );
        pass.set_bind_group(1, binding, &[]);
        RenderCommandResult::Success
    }
}

/// Binds one pass of the extrusion pipeline and the item's view or flat projection.
pub struct SetExtrusionPipeline<const PASS: u8>;
/// The pass an extrusion draw belongs to.
pub mod extrusion_pass {
    /// Depth only.
    pub const DEPTH: u8 = 0;
    /// Colour once per pixel.
    pub const COLOR: u8 = 1;
    /// Stencil reset.
    pub const CLEAR: u8 = 2;
}

impl<P: PhaseItem, const PASS: u8> RenderCommand<P> for SetExtrusionPipeline<PASS> {
    fn render<'w>(
        world: &'w World,
        item: &P,
        pass: &mut wgpu::RenderPass<'w>,
    ) -> RenderCommandResult {
        let Some((Initialized(pipeline), Initialized(projection_resources))) =
            world.resources.query::<(
                &Eventually<ExtrusionPipeline>,
                &Eventually<ProjectionGpuResources>,
            )>()
        else {
            return RenderCommandResult::Failure;
        };

        pass.set_pipeline(match PASS {
            extrusion_pass::DEPTH => &pipeline.depth,
            extrusion_pass::COLOR => &pipeline.color,
            _ => &pipeline.clear,
        });
        pass.set_bind_group(
            0,
            projection_resources.bind_group_for(item.projection_binding()),
            &[],
        );
        RenderCommandResult::Success
    }
}

/// Binds and draws a polygon bucket filled with a repeating image.
pub type DrawPatternTiles = (SetPatternTilePipeline, DrawVectorTile);
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
/// Resets the stencil bit the colour draw marked.
pub type DrawExtrusionClear = (
    SetExtrusionPipeline<{ extrusion_pass::CLEAR }>,
    DrawVectorTile,
);
/// Binds and draws a polygon-fill bucket when both commands succeed.
pub type DrawVectorTiles = (SetVectorTilePipeline, DrawVectorTile);
/// Binds and draws a stroked-line bucket when both commands succeed.
pub type DrawLineTiles = (SetLineTilePipeline, DrawVectorTile);
/// Binds and draws a circle bucket when both commands succeed.
pub type DrawCircleTiles = (SetCircleTilePipeline, DrawVectorTile);

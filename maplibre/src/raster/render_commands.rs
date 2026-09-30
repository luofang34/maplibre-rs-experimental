//! Raster draw commands that stop when uploaded textures, meshes or metadata are missing.

use crate::{
    raster::resource::RasterResources,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        projection::ProjectionGpuResources,
        render_phase::{LayerItem, PhaseItem, RenderCommand, RenderCommandResult},
        tile_mesh::{GlobeTileMeshCache, TileMeshUsage},
        tile_view_pattern::WgpuTileViewPattern,
    },
    tcs::world::World,
};

/// Binds the raster pipeline and the item's view or flat projection at group zero.
pub struct SetRasterTilePipeline;
impl<P: PhaseItem> RenderCommand<P> for SetRasterTilePipeline {
    fn render<'w>(
        world: &'w World,
        item: &P,
        pass: &mut wgpu::RenderPass<'w>,
    ) -> RenderCommandResult {
        let Some((Initialized(raster_resources), Initialized(projection_resources))) =
            world.resources.query::<(
                &Eventually<RasterResources>,
                &Eventually<ProjectionGpuResources>,
            )>()
        else {
            return RenderCommandResult::Failure;
        };

        pass.set_pipeline(raster_resources.pipeline());
        pass.set_bind_group(
            0,
            projection_resources.bind_group_for(item.projection_binding()),
            &[],
        );
        RenderCommandResult::Success
    }
}

/// Binds the tile's uploaded texture and sampler at the pipeline's fixed group one.
pub struct SetRasterViewBindGroup;
impl RenderCommand<LayerItem> for SetRasterViewBindGroup {
    fn render<'w>(
        world: &'w World,
        item: &LayerItem,
        pass: &mut wgpu::RenderPass<'w>,
    ) -> RenderCommandResult {
        let Some(Initialized(raster_resources)) =
            world.resources.get::<Eventually<RasterResources>>()
        else {
            return RenderCommandResult::Failure;
        };

        let Some(bind_group) = raster_resources.layer_texture(&item.style_layer, &item.tile.coords)
        else {
            return RenderCommandResult::Failure;
        };

        pass.set_bind_group(1, bind_group, &[]);
        RenderCommandResult::Success
    }
}

/// Binds the layer's paint adjustments at group two.
pub struct SetRasterPaintBindGroup;
impl RenderCommand<LayerItem> for SetRasterPaintBindGroup {
    fn render<'w>(
        world: &'w World,
        item: &LayerItem,
        pass: &mut wgpu::RenderPass<'w>,
    ) -> RenderCommandResult {
        let Some(Initialized(raster_resources)) =
            world.resources.get::<Eventually<RasterResources>>()
        else {
            return RenderCommandResult::Failure;
        };
        let Some(bind_group) = raster_resources.layer_paint(&item.style_layer) else {
            return RenderCommandResult::Failure;
        };
        pass.set_bind_group(2, bind_group, &[]);
        RenderCommandResult::Success
    }
}

/// Draws a cached tile mesh with the item's stencil reference and current metadata range.
pub struct DrawRasterTile;
impl RenderCommand<LayerItem> for DrawRasterTile {
    fn render<'w>(
        world: &'w World,
        item: &LayerItem,
        pass: &mut wgpu::RenderPass<'w>,
    ) -> RenderCommandResult {
        let Some((Initialized(tile_view_pattern), tile_mesh_cache)) = world
            .resources
            .query::<(&Eventually<WgpuTileViewPattern>, &GlobeTileMeshCache)>()
        else {
            return RenderCommandResult::Failure;
        };

        let source_shape = &item.source_shape;
        let Some(mesh) = tile_mesh_cache.get(
            source_shape.coords(),
            TileMeshUsage::Raster,
            item.generate_borders,
        ) else {
            return RenderCommandResult::Failure;
        };

        let reference = source_shape.coords().stencil_reference_value_3d() as u32;

        pass.set_stencil_reference(reference);

        let Some(tile_view_pattern_buffer) = source_shape.buffer_range() else {
            return RenderCommandResult::Failure;
        };
        pass.set_vertex_buffer(0, mesh.vertex_buffer().slice(..));
        pass.set_vertex_buffer(
            1,
            tile_view_pattern.buffer().slice(tile_view_pattern_buffer),
        );

        pass.set_index_buffer(mesh.index_buffer().slice(..), mesh.index_format());
        pass.draw_indexed(0..mesh.index_count(), 0, 0..1);

        RenderCommandResult::Success
    }
}

/// Binds the raster pipeline and image, then draws only when every command succeeds.
pub type DrawRasterTiles = (
    SetRasterTilePipeline,
    SetRasterViewBindGroup,
    SetRasterPaintBindGroup,
    DrawRasterTile,
);

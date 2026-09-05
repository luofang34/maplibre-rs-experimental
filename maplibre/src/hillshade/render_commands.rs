//! Render commands of the DEM-shaded layers.

use crate::{
    hillshade::resources::HillshadeResources,
    raster::render_commands::{DrawRasterTile, SetRasterViewBindGroup},
    render::{
        eventually::{Eventually, Eventually::Initialized},
        projection::ProjectionGpuResources,
        render_phase::{LayerItem, RenderCommand, RenderCommandResult},
        resource::TrackedRenderPass,
    },
    tcs::world::World,
};

/// Binds the pipeline of the item's layer kind and the projection.
pub struct SetDemPipeline;
impl RenderCommand<LayerItem> for SetDemPipeline {
    fn render<'w>(
        world: &'w World,
        item: &LayerItem,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some((Initialized(resources), Initialized(projection_resources))) =
            world.resources.query::<(
                &Eventually<HillshadeResources>,
                &Eventually<ProjectionGpuResources>,
            )>()
        else {
            return RenderCommandResult::Failure;
        };
        let Some((kind, _)) = resources.layer(&item.style_layer) else {
            return RenderCommandResult::Failure;
        };
        pass.set_render_pipeline(resources.pipeline(kind));
        pass.set_bind_group(0, projection_resources.bind_group_for(item.projection), &[]);
        RenderCommandResult::Success
    }
}

/// Binds the layer's uniforms at group 2.
pub struct SetDemLayerBindGroup;
impl RenderCommand<LayerItem> for SetDemLayerBindGroup {
    fn render<'w>(
        world: &'w World,
        item: &LayerItem,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some(Initialized(resources)) = world.resources.get::<Eventually<HillshadeResources>>()
        else {
            return RenderCommandResult::Failure;
        };
        let Some((_, bind_group)) = resources.layer(&item.style_layer) else {
            return RenderCommandResult::Failure;
        };
        pass.set_bind_group(2, bind_group, &[]);
        RenderCommandResult::Success
    }
}

/// Draws one DEM tile shape with the layer's shading.
pub type DrawDemTiles = (
    SetDemPipeline,
    SetRasterViewBindGroup<0>,
    SetDemLayerBindGroup,
    DrawRasterTile,
);

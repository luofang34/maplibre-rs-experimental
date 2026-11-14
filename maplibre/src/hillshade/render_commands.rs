//! Render commands of the DEM-shaded layers.

#[cfg(feature = "raster")]
use crate::raster::{render_commands::DrawRasterTile, resource::RasterResources};
use crate::{
    hillshade::resources::HillshadeResources,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        projection::ProjectionGpuResources,
        render_phase::{LayerItem, RenderCommand, RenderCommandResult},
    },
    tcs::world::World,
};

/// Binds the pipeline of the item's layer kind and the projection.
pub struct SetDemPipeline;
impl RenderCommand<LayerItem> for SetDemPipeline {
    fn render<'w>(
        world: &'w World,
        item: &LayerItem,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
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
        pass.set_pipeline(resources.pipeline(kind));
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
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
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

/// Binds the DEM tile texture at group 1 using the layer's resampling filter (`linear` or `nearest`).
#[cfg(feature = "raster")]
pub struct SetDemViewBindGroup;
#[cfg(feature = "raster")]
impl RenderCommand<LayerItem> for SetDemViewBindGroup {
    fn render<'w>(
        world: &'w World,
        item: &LayerItem,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some((Initialized(hillshade), Initialized(raster_resources))) =
            world.resources.query::<(
                &Eventually<HillshadeResources>,
                &Eventually<RasterResources>,
            )>()
        else {
            return RenderCommandResult::Failure;
        };
        let nearest = hillshade.layer_nearest(&item.style_layer);
        let Some(bind_group) =
            raster_resources.layer_texture_filtered(&item.style_layer, &item.tile.coords, nearest)
        else {
            return RenderCommandResult::Failure;
        };
        pass.set_bind_group(1, bind_group, &[]);
        RenderCommandResult::Success
    }
}

/// Draws one DEM tile shape with the layer's shading.
#[cfg(feature = "raster")]
pub type DrawDemTiles = (
    SetDemPipeline,
    SetDemViewBindGroup,
    SetDemLayerBindGroup,
    DrawRasterTile,
);

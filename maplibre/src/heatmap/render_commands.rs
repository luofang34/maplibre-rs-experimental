//! Render commands of the heatmap density draws and the composite.

use crate::{
    heatmap::resources::HeatmapResources,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        projection::ProjectionGpuResources,
        render_phase::{LayerItem, RenderCommand, RenderCommandResult},
    },
    tcs::world::World,
    vector::render_commands::DrawVectorTile,
};

/// Binds the density pipeline and the item's projection.
pub struct SetHeatmapDensityPipeline;
impl RenderCommand<LayerItem> for SetHeatmapDensityPipeline {
    fn render<'w>(
        world: &'w World,
        item: &LayerItem,
        pass: &mut wgpu::RenderPass<'w>,
    ) -> RenderCommandResult {
        let Some((Initialized(resources), Initialized(projection_resources))) =
            world.resources.query::<(
                &Eventually<HeatmapResources>,
                &Eventually<ProjectionGpuResources>,
            )>()
        else {
            return RenderCommandResult::Failure;
        };
        pass.set_pipeline(resources.density_pipeline());
        pass.set_bind_group(0, projection_resources.bind_group_for(item.projection), &[]);
        RenderCommandResult::Success
    }
}

/// Binds the composite pipeline with the layer's density and ramp, then covers the viewport.
pub struct DrawHeatmapComposite;
impl RenderCommand<LayerItem> for DrawHeatmapComposite {
    fn render<'w>(
        world: &'w World,
        item: &LayerItem,
        pass: &mut wgpu::RenderPass<'w>,
    ) -> RenderCommandResult {
        let Some(Initialized(resources)) = world.resources.get::<Eventually<HeatmapResources>>()
        else {
            return RenderCommandResult::Failure;
        };
        let (Some(density), Some(ramp)) = (
            resources.density_bind_group(&item.style_layer),
            resources.ramp_bind_group(&item.style_layer),
        ) else {
            return RenderCommandResult::Failure;
        };
        pass.set_pipeline(resources.composite_pipeline());
        pass.set_bind_group(0, density, &[]);
        pass.set_bind_group(1, ramp, &[]);
        pass.draw(0..3, 0..1);
        RenderCommandResult::Success
    }
}

/// Adds one tile's points of a layer into its density target.
pub type DrawHeatmapDensityTiles = (SetHeatmapDensityPipeline, DrawVectorTile);

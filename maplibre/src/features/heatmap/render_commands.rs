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
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some((Initialized(resources), Initialized(projection_resources))) =
            world.resources.query::<(
                &Eventually<HeatmapResources>,
                &Eventually<ProjectionGpuResources>,
            )>()
        else {
            return RenderCommandResult::Failure;
        };
        resources.bind_density(pass, projection_resources.bind_group_for(item.projection));
        RenderCommandResult::Success
    }
}

/// Binds the composite pipeline with the layer's density and ramp, then covers the viewport.
pub struct DrawHeatmapComposite;
impl RenderCommand<LayerItem> for DrawHeatmapComposite {
    fn render<'w>(
        world: &'w World,
        item: &LayerItem,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some(Initialized(resources)) = world.resources.get::<Eventually<HeatmapResources>>()
        else {
            return RenderCommandResult::Failure;
        };
        if resources.draw_composite(item.style_layer.as_str(), pass) {
            RenderCommandResult::Success
        } else {
            RenderCommandResult::Failure
        }
    }
}

/// Adds one tile's points of a layer into its density target.
pub type DrawHeatmapDensityTiles = (SetHeatmapDensityPipeline, DrawVectorTile);

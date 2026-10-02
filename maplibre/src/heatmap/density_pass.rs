//! Renders each heatmap layer's kernels into its density target before the main pass.

use wgpu::StoreOp;

use crate::{
    heatmap::{queue_system::HeatmapDensityPhase, resources::HeatmapResources},
    render::{
        eventually::{Eventually, Eventually::Initialized},
        graph::{Node, NodeRunError, RenderContext, RenderGraphContext},
        RenderResources,
    },
    tcs::world::World,
};

/// Name of the density node inside the draw sub-graph.
pub const DENSITY_PASS: &str = "heatmap_density_pass";

/// Render-graph node running one render pass per heatmap layer.
pub struct DensityPassNode;

impl Node for DensityPassNode {
    fn run(
        &self,
        _graph: &mut RenderGraphContext,
        render_context: &mut RenderContext,
        _resources: &RenderResources,
        world: &World,
    ) -> Result<(), NodeRunError> {
        let Some(Initialized(resources)) = world.resources.get::<Eventually<HeatmapResources>>()
        else {
            return Ok(());
        };
        let Some(phase) = world.resources.get::<HeatmapDensityPhase>() else {
            return Ok(());
        };
        for layer in &phase.layers {
            let Some(view) = resources.density_view(&layer.layer) else {
                continue;
            };
            // A layer without points still clears its target, or last frame's density stays.
            let pass =
                render_context
                    .command_encoder
                    .begin_render_pass(&wgpu::RenderPassDescriptor {
                        multiview_mask: None,
                        label: Some("heatmap_density_pass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            depth_slice: None,
                            view,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                                store: StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                    });
            let mut pass = crate::render::tracked_pass::TrackedRenderPass::new(pass);
            for item in &layer.items {
                item.draw_function.draw(&mut pass, world, item);
            }
            pass.finish(world);
        }
        Ok(())
    }
}

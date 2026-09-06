//! Renders the drapeable layers of each view tile into that tile's texture.

use wgpu::StoreOp;

use crate::{
    render::{
        eventually::{Eventually, Eventually::Initialized},
        graph::{Node, NodeRunError, RenderContext, RenderGraphContext},
        resource::TrackedRenderPass,
        RenderResources,
    },
    tcs::world::World,
    terrain::{resources::TerrainResources, DrapePhase},
};

/// Name of the drape node inside the draw sub-graph.
pub const DRAPE_PASS: &str = "drape_pass";

/// Render-graph node running one render pass per drape target before the main pass.
pub struct DrapePassNode;

impl Node for DrapePassNode {
    fn run(
        &self,
        _graph: &mut RenderGraphContext,
        render_context: &mut RenderContext,
        _resources: &RenderResources,
        world: &World,
    ) -> Result<(), NodeRunError> {
        let Some(Initialized(terrain)) = world.resources.get::<Eventually<TerrainResources>>()
        else {
            return Ok(());
        };
        let Some(phase) = world.resources.get::<DrapePhase>() else {
            return Ok(());
        };
        let Some(scratch) = terrain.scratch() else {
            return Ok(());
        };
        tracing::trace!(targets = phase.targets.len(), "drape pass");
        for target in &phase.targets {
            let Some(texture) = terrain.drape_texture(target.coords) else {
                continue;
            };
            let ops = wgpu::Operations {
                load: wgpu::LoadOp::Clear(target.clear_color),
                store: StoreOp::Store,
            };
            // The layers are drawn into the first level alone; the sampled view spans every
            // level, which an attachment may not.
            let top_level = texture.texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("drape top level"),
                base_mip_level: 0,
                mip_level_count: Some(1),
                ..Default::default()
            });
            let color_attachment = match &scratch.color {
                Some(multisampled) => wgpu::RenderPassColorAttachment {
                    view: &multisampled.view,
                    resolve_target: Some(&top_level),
                    ops,
                },
                None => wgpu::RenderPassColorAttachment {
                    view: &top_level,
                    resolve_target: None,
                    ops,
                },
            };
            let pass =
                render_context
                    .command_encoder
                    .begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("drape_pass"),
                        color_attachments: &[Some(color_attachment)],
                        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                            view: &scratch.depth_stencil.view,
                            depth_ops: Some(wgpu::Operations {
                                load: wgpu::LoadOp::Clear(0.0),
                                store: StoreOp::Discard,
                            }),
                            stencil_ops: Some(wgpu::Operations {
                                load: wgpu::LoadOp::Clear(0),
                                store: StoreOp::Discard,
                            }),
                        }),
                        timestamp_writes: None,
                        occlusion_query_set: None,
                    });
            {
                let mut tracked_pass = TrackedRenderPass::new(pass);
                for mask in &target.masks {
                    mask.draw_function.draw(&mut tracked_pass, world, mask);
                }
                for layer in &target.layers {
                    layer.draw_function.draw(&mut tracked_pass, world, layer);
                }
            }
            terrain.generate_drape_mipmaps(
                render_context.device,
                &mut render_context.command_encoder,
                texture,
            );
        }
        Ok(())
    }
}

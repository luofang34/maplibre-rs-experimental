//! Renders the drapeable layers of each view tile into that tile's texture.

use wgpu::StoreOp;

use crate::{
    render::{
        eventually::{Eventually, Eventually::Initialized},
        graph::{Node, NodeRunError, RenderContext, RenderGraphContext},
        render_commands::{DrawMask, SetMaskPipeline},
        render_phase::{RenderCommand, RenderCommandResult},
        RenderResources,
    },
    tcs::world::World,
    terrain::{
        resources::{DrapeScratch, TerrainResources},
        DrapePhase, DrapeTarget,
    },
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
            // The layers are drawn into the first level alone; the sampled view spans every
            // level, which an attachment may not.
            let top_level = texture.texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("drape top level"),
                base_mip_level: 0,
                mip_level_count: Some(1),
                ..Default::default()
            });
            let color_attachment = color_attachment(scratch, &top_level, target.clear_color);
            let pass =
                render_context
                    .command_encoder
                    .begin_render_pass(&wgpu::RenderPassDescriptor {
                        multiview_mask: None,
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
                let mut render_pass = crate::render::tracked_pass::TrackedRenderPass::new(pass);
                draw_layers(&mut render_pass, world, target);
                render_pass.finish(world);
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

fn draw_layers<'w>(
    pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    world: &'w World,
    target: &DrapeTarget,
) {
    for layers in target
        .layers
        .chunk_by(|left, right| left.index == right.index)
    {
        let Some(first_mask) = target.masks.first() else {
            continue;
        };
        if matches!(
            SetMaskPipeline::render(world, first_mask, pass),
            RenderCommandResult::Failure
        ) {
            continue;
        }
        // The source of a layer can use a different tile pyramid. Old stencil values must
        // not clip its geometry or admit buffered geometry outside the current tile masks.
        for mask in &target.masks {
            DrawMask::render_with_reference(world, mask, pass, 0);
        }
        for mask in &target.masks {
            if layers
                .iter()
                .any(|layer| layer.source_shape.buffer_range() == mask.source_shape.buffer_range())
            {
                mask.draw_function.draw(pass, world, mask);
            }
        }
        for layer in layers {
            layer.draw_function.draw(pass, world, layer);
        }
    }
}

fn color_attachment<'a>(
    scratch: &'a DrapeScratch,
    top_level: &'a wgpu::TextureView,
    color: wgpu::Color,
) -> wgpu::RenderPassColorAttachment<'a> {
    let ops = wgpu::Operations {
        load: wgpu::LoadOp::Clear(color),
        store: StoreOp::Store,
    };
    match &scratch.color {
        Some(multisampled) => wgpu::RenderPassColorAttachment {
            depth_slice: None,
            view: &multisampled.view,
            resolve_target: Some(top_level),
            ops,
        },
        None => wgpu::RenderPassColorAttachment {
            depth_slice: None,
            view: top_level,
            resolve_target: None,
            ops,
        },
    }
}

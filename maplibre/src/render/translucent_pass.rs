use std::ops::Deref;

use wgpu::StoreOp;

use crate::{
    render::{
        draw_graph,
        graph::{Node, NodeRunError, RenderContext, RenderGraphContext, SlotInfo},
        render_phase::{RenderPhase, TranslucentItem},
        resource::TrackedRenderPass,
        Eventually::Initialized,
        RenderResources,
    },
    tcs::world::World,
};

pub struct TranslucentPassNode {}

impl TranslucentPassNode {
    pub fn new() -> Self {
        Self {}
    }
}

impl Node for TranslucentPassNode {
    fn input(&self) -> Vec<SlotInfo> {
        vec![]
    }

    fn update(&mut self, _state: &mut RenderResources) {}

    fn run(
        &self,
        _graph: &mut RenderGraphContext,
        render_context: &mut RenderContext,
        state: &RenderResources,
        world: &World,
    ) -> Result<(), NodeRunError> {
        let Initialized(render_target) = &state.render_target else {
            return Ok(());
        };
        let Initialized(multisampling_texture) = &state.multisampling_texture else {
            return Ok(());
        };

        let Initialized(depth_texture) = &state.depth_texture else {
            return Ok(());
        };
        if let Some(Initialized(symbol_depth)) =
            world
                .resources
                .get::<crate::render::eventually::Eventually<crate::sdf::depth::SymbolDepth>>()
        {
            symbol_depth.copy.copy(
                render_context.device,
                &mut render_context.command_encoder,
                &depth_texture.texture,
                &symbol_depth.view,
            );
        }
        let color_attachment = if let Some(texture) = multisampling_texture {
            wgpu::RenderPassColorAttachment {
                view: &texture.view,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: StoreOp::Store,
                },
                resolve_target: Some(render_target.deref()),
            }
        } else {
            wgpu::RenderPassColorAttachment {
                view: render_target.deref(),
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: StoreOp::Store,
                },
                resolve_target: None,
            }
        };

        let render_pass =
            render_context
                .command_encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("translucent_pass"),
                    color_attachments: &[Some(color_attachment)],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &depth_texture.view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: StoreOp::Store,
                        }),
                        stencil_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: StoreOp::Store,
                        }),
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });

        let mut tracked_pass = TrackedRenderPass::new(render_pass);

        // TODO: Automatically raise error when items get linearly too many (+1k)

        if let Some(mask_items) = world.resources.get::<RenderPhase<TranslucentItem>>() {
            log::trace!(
                "RenderPhase<TranslucentItem>::size() = {}",
                mask_items.size()
            );
            for item in mask_items {
                item.draw_function.draw(&mut tracked_pass, world, item);
            }
        }

        Ok(())
    }
}

pub struct MainPassDriverNode;

impl Node for MainPassDriverNode {
    fn run(
        &self,
        graph: &mut RenderGraphContext,
        _render_context: &mut RenderContext,
        _resources: &RenderResources,
        _world: &World,
    ) -> Result<(), NodeRunError> {
        graph.run_sub_graph(draw_graph::NAME, vec![])?;

        Ok(())
    }
}

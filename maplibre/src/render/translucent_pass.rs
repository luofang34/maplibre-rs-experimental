use std::ops::Deref;

use wgpu::StoreOp;

use crate::{
    render::{
        graph::{Node, RenderContext},
        render_phase::{RenderPhase, TranslucentItem},
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
    fn run(&self, render_context: &mut RenderContext, state: &RenderResources, world: &World) {
        let Initialized(render_target) = &state.render_target else {
            return;
        };
        let Initialized(multisampling_texture) = &state.multisampling_texture else {
            return;
        };

        let Initialized(depth_texture) = &state.depth_texture else {
            return;
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
                depth_slice: None,
                view: &texture.view,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    // Resolved into `render_target`; discarding the transient MSAA surface avoids
                    // writing multisampled tile memory back to DRAM on mobile TBDR GPUs.
                    store: StoreOp::Discard,
                },
                resolve_target: Some(render_target.deref()),
            }
        } else {
            wgpu::RenderPassColorAttachment {
                depth_slice: None,
                view: render_target.deref(),
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: StoreOp::Store,
                },
                resolve_target: None,
            }
        };

        let needs_depth_after_pass = state.eye_depth_target.is_some()
            || matches!(state.depth_readout_texture, Initialized(_));
        let render_pass =
            render_context
                .command_encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    multiview_mask: None,
                    label: Some("translucent_pass"),
                    color_attachments: &[Some(color_attachment)],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &depth_texture.view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: if needs_depth_after_pass {
                                StoreOp::Store
                            } else {
                                StoreOp::Discard
                            },
                        }),
                        stencil_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: if needs_depth_after_pass {
                                StoreOp::Store
                            } else {
                                StoreOp::Discard
                            },
                        }),
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
        let mut render_pass = crate::render::tracked_pass::TrackedRenderPass::new(render_pass);
        // Pass hooks at the beginning of translucent pass (e.g. sun, moon).
        let pass_hooks = world
            .resources
            .get::<crate::render::PassDrawHooks>()
            .map(|hooks| hooks.get_hooks(crate::render::PassHookPoint::TranslucentPassBegin));
        if let Some(hooks) = pass_hooks {
            for hook in hooks {
                hook.draw(&mut render_pass, world);
            }
        }

        // TODO: Automatically raise error when items get linearly too many (+1k)

        if let Some(mask_items) = world.resources.get::<RenderPhase<TranslucentItem>>() {
            log::trace!(
                "RenderPhase<TranslucentItem>::size() = {}",
                mask_items.size()
            );
            let above = super::main_pass::above_symbols(world);
            let mut previous = above.as_ref().map_or(&[][..], |above| above.previous);
            let mut groups = above
                .as_ref()
                .map_or(&[][..], |above| above.items)
                .chunk_by(super::main_pass::same_layer)
                .peekable();
            for item in mask_items {
                while let Some(group) = groups.next_if(|group| group[0].index < item.index) {
                    super::main_pass::draw_group(&mut render_pass, world, previous, group);
                    previous = group;
                }
                item.draw_function.draw(&mut render_pass, world, item);
            }
            for group in groups {
                super::main_pass::draw_group(&mut render_pass, world, previous, group);
                previous = group;
            }
        }
        let end_hooks = world
            .resources
            .get::<crate::render::PassDrawHooks>()
            .map(|hooks| hooks.get_hooks(crate::render::PassHookPoint::TranslucentPassEnd));
        if let Some(hooks) = end_hooks {
            for hook in hooks {
                hook.draw(&mut render_pass, world);
            }
        }
        render_pass.finish(world);
    }
}

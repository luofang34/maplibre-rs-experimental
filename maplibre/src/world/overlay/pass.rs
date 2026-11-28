//! The pass that draws the host's overlay over the finished map.

use wgpu::StoreOp;

use super::pipeline::OverlayGpu;
use crate::{
    render::{
        eventually::Eventually::{self, Initialized},
        graph::{Node, RenderContext},
        RenderResources,
    },
    tcs::world::World,
};

/// The overlay pass's node in the draw graph, after the main pass and before the symbols.
pub const OVERLAY_PASS: &str = "overlay_pass";

/// Draws the host's mesh onto the map's colour target, keeping what the map drew.
pub(super) struct OverlayPassNode;

impl Node for OverlayPassNode {
    fn writes_custom_scene_depth(&self) -> bool {
        false
    }

    fn has_active_work(&self, state: &RenderResources, world: &World) -> bool {
        matches!(
            (
                world.resources.get::<Eventually<OverlayGpu>>(),
                &state.render_target,
                &state.multisampling_texture,
                &state.depth_texture,
            ),
            (
                Some(Initialized(gpu)),
                Initialized(_),
                Initialized(_),
                Initialized(_),
            ) if gpu.has_mesh()
        )
    }

    fn run(&self, render_context: &mut RenderContext, state: &RenderResources, world: &World) {
        let Some(Initialized(gpu)) = world.resources.get::<Eventually<OverlayGpu>>() else {
            return;
        };
        if !gpu.has_mesh() {
            return;
        }
        let Initialized(render_target) = &state.render_target else {
            return;
        };
        let Initialized(multisampling_texture) = &state.multisampling_texture else {
            return;
        };
        let Initialized(depth_texture) = &state.depth_texture else {
            return;
        };
        let has_stencil = depth_texture.as_ref().format().has_stencil_aspect();
        // As the symbol pass after it does: draw into the multisampled target and resolve.
        let (view, resolve_target) = match multisampling_texture {
            Some(texture) => (&**texture, Some(&**render_target)),
            None => (&**render_target, None),
        };
        let mut pass =
            render_context
                .command_encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    multiview_mask: None,
                    label: Some(OVERLAY_PASS),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        depth_slice: None,
                        view,
                        resolve_target,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: StoreOp::Store,
                        },
                    })],
                    // The frame's depth, read to hide what terrain covers.
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: depth_texture,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: StoreOp::Store,
                        }),
                        stencil_ops: has_stencil.then_some(wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: StoreOp::Store,
                        }),
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
        gpu.draw(&mut pass);
    }
}

//! The pass that draws the host's overlay over the finished map.

use wgpu::StoreOp;

use super::pipeline::OverlayGpu;
use crate::{
    render::{
        eventually::Eventually::{self, Initialized},
        graph::{Node, NodeRunError, RenderContext, RenderGraphContext},
        RenderResources,
    },
    tcs::world::World,
};

/// The overlay pass's node in the draw graph, after the main pass and before the symbols.
pub const OVERLAY_PASS: &str = "overlay_pass";

/// Draws the host's mesh onto the map's colour target, keeping what the map drew.
pub(super) struct OverlayPassNode;

impl Node for OverlayPassNode {
    fn run(
        &self,
        _graph: &mut RenderGraphContext,
        render_context: &mut RenderContext,
        state: &RenderResources,
        world: &World,
    ) -> Result<(), NodeRunError> {
        let Some(Initialized(gpu)) = world.resources.get::<Eventually<OverlayGpu>>() else {
            return Ok(());
        };
        let Some(mesh) = &gpu.mesh else {
            return Ok(());
        };
        let Initialized(render_target) = &state.render_target else {
            return Ok(());
        };
        let Initialized(multisampling_texture) = &state.multisampling_texture else {
            return Ok(());
        };
        // As the symbol pass after it does: draw into the multisampled target and resolve.
        let (view, resolve_target) = match multisampling_texture {
            Some(texture) => (&*texture.view, Some(&**render_target)),
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
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
        pass.set_pipeline(&gpu.pipeline);
        pass.set_bind_group(0, &gpu.bind_group, &[]);
        pass.set_vertex_buffer(0, mesh.vertices.slice(..));
        pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..mesh.count, 0, 0..1);
        Ok(())
    }
}

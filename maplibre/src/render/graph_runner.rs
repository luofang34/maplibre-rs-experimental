//! Records the frame's render passes into one command buffer and submits it.

#![deny(missing_docs)]

use crate::{
    render::{
        graph::{RenderContext, RenderGraph, RenderGraphError},
        AfterSubmitCallbacks, BeforeSubmitHooks, RenderResources,
    },
    tcs::world::World,
};

/// Records every pass, runs the [`BeforeSubmitHooks`], submits the frame and runs the
/// [`AfterSubmitCallbacks`]. Nothing is submitted when the pass order cannot be resolved.
pub(crate) fn run_graph(
    graph: &RenderGraph,
    device: &wgpu::Device,
    queue: &crate::render::upload_queue::UploadQueue,
    state: &RenderResources,
    world: &World,
) -> Result<(), RenderGraphError> {
    let mut render_context = RenderContext {
        device,
        command_encoder: device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default()),
    };
    graph.run(&mut render_context, state, world)?;
    if let Some(hooks) = world.resources.get::<BeforeSubmitHooks>() {
        hooks.run(world, &mut render_context.command_encoder);
    }
    {
        #[cfg(feature = "trace")]
        let _span = tracing::info_span!("submit_graph_commands").entered();
        queue.submit(vec![render_context.command_encoder.finish()]);
    }
    if let Some(callbacks) = world.resources.get::<AfterSubmitCallbacks>() {
        callbacks.run(world);
    }
    Ok(())
}

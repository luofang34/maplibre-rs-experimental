//! Stars behind the globe, drawn from the directions, magnitudes and colours a host sets with
//! [`crate::context::MapContext::set_stars`].
//!
//! Stars lie at infinity: the globe's view-projection carries their directions with w = 0, so
//! they turn with the camera and never move with it. They are drawn first in the main pass at
//! the far plane and test depth without writing it, so the globe, its terrain and every layer
//! cover them. A globe whose style paints nothing on it, with no background and no terrain,
//! shows the stars through its disc. Where the atmosphere is drawn, each star fades with the
//! brightness of the sky in its direction, so a lit sky hides the stars a night sky shows.

use crate::{
    environment::Environment,
    plugin::Plugin,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        projection::ProjectionGpuResources,
        tracked_pass::TrackedRenderPass,
        RenderStageLabel,
    },
    tcs::world::World,
};

mod pipeline;
mod upload;

pub use pipeline::StarfieldPipeline;
pub use upload::{star_instance, StarInstance, StarfieldGpu};

/// Draws the host's stars behind a globe. Build it after [`crate::render::RenderPlugin`].
#[derive(Default)]
pub struct StarfieldPlugin;

impl<E: Environment> Plugin<E> for StarfieldPlugin {
    fn build(
        &self,
        schedule: &mut crate::schedule::Schedule,
        _kernel: std::rc::Rc<crate::kernel::Kernel<E>>,
        world: &mut World,
        _graph: &mut crate::render::graph::RenderGraph,
    ) {
        world
            .resources
            .insert(Eventually::<StarfieldPipeline>::Uninitialized);
        schedule.add_system_to_stage(RenderStageLabel::Prepare, pipeline::resource_system);
        schedule.add_system_to_stage(RenderStageLabel::Queue, upload::queue_system);
    }
}

/// Draws the stars queued for this frame, if any; the main pass calls it before any layer.
pub fn draw<'w>(pass: &mut TrackedRenderPass<'w>, world: &'w World) {
    let Some((Initialized(StarfieldPipeline(pipeline)), Initialized(projection), gpu)) =
        world.resources.query::<(
            &Eventually<StarfieldPipeline>,
            &Eventually<ProjectionGpuResources>,
            &StarfieldGpu,
        )>()
    else {
        return;
    };
    let Some((instances, frame)) = gpu.drawable() else {
        return;
    };
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, projection.bind_group(), &[]);
    pass.set_bind_group(1, frame, &[]);
    pass.set_vertex_buffer(0, instances.slice(..));
    pass.draw(0..6, 0..gpu.count());
}

#[cfg(test)]
mod tests;

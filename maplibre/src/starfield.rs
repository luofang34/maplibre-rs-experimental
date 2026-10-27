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

pub mod moon;
mod pipeline;
pub mod sun;
mod upload;

pub use pipeline::StarfieldPipeline;
pub use upload::{star_instance, StarInstance, StarfieldGpu};

/// Draws the host's stars, sun and moon behind a globe. Build it after
/// [`crate::render::RenderPlugin`].
#[derive(Default)]
pub struct StarfieldPlugin;

impl<E: Environment> Plugin<E> for StarfieldPlugin {
    fn build(
        &self,
        schedule: &mut crate::schedule::Schedule,
        _kernel: std::rc::Rc<crate::kernel::Kernel<E>>,
        world: &mut World,
        graph: &mut crate::render::graph::RenderGraph,
    ) {
        world
            .resources
            .insert(Eventually::<StarfieldPipeline>::Uninitialized);
        world
            .resources
            .insert(Eventually::<sun::SunPipeline>::Uninitialized);
        world
            .resources
            .insert(Eventually::<sun::depth::SunDepth>::Uninitialized);
        world
            .resources
            .insert(Eventually::<moon::MoonPipeline>::Uninitialized);
        schedule.add_system_to_stage(RenderStageLabel::Prepare, pipeline::resource_system);
        schedule.add_system_to_stage(RenderStageLabel::Prepare, sun::resource_system);
        schedule.add_system_to_stage(RenderStageLabel::Prepare, moon::resource_system);
        schedule.add_system_to_stage(RenderStageLabel::Queue, upload::queue_system);
        schedule.add_system_to_stage(RenderStageLabel::Queue, sun::queue_system);
        schedule.add_system_to_stage(RenderStageLabel::Queue, moon::queue_system);
        add_sun_depth_copy(graph);
    }
}

/// Puts the sun's depth copy between the main and translucent passes, after everything the
/// main pass draws and before the sun is drawn.
fn add_sun_depth_copy(graph: &mut crate::render::graph::RenderGraph) {
    use crate::render::draw_graph;
    let Some(draw_graph) = graph.get_sub_graph_mut(draw_graph::NAME) else {
        tracing::error!("draw graph is missing; the sun's glare will not see the ground");
        return;
    };
    draw_graph.add_node(sun::depth::SUN_DEPTH_COPY, sun::depth::SunDepthCopyNode);
    let edges = [
        draw_graph.add_node_edge(draw_graph::node::MAIN_PASS, sun::depth::SUN_DEPTH_COPY),
        draw_graph.add_node_edge(
            sun::depth::SUN_DEPTH_COPY,
            draw_graph::node::TRANSLUCENT_PASS,
        ),
    ];
    for edge in edges {
        if let Err(error) = edge {
            tracing::error!(?error, "the sun's depth copy cannot be ordered");
        }
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

//! Heatmap layers: point kernels accumulated into an offscreen density texture, then coloured
//! through the layer's ramp and composited in style order.
//!
//! The vector tile path tessellates, uploads and buffers heatmap points like circles. Each
//! frame a density pass draws every visible layer's points into that layer's half-float
//! target with additive blending, and a fullscreen composite in the main pass turns the
//! density into colour at the layer's place in the style. With terrain the heatmap still
//! draws to the screen, and globe rendering is not supported.

use std::rc::Rc;

use crate::{
    environment::Environment,
    kernel::Kernel,
    plugin::Plugin,
    render::{
        draw_graph,
        eventually::Eventually,
        graph::{NodeLabel, RenderGraph},
        RenderStageLabel,
    },
    schedule::Schedule,
    tcs::world::World,
};

mod density_pass;
mod prepare_system;
mod queue_system;
pub mod render_commands;
mod resource_system;
pub mod resources;

pub use queue_system::HeatmapDensityPhase;
pub use resources::HeatmapResources;

/// Registers the `heatmap` layer type; the vector plugin must be registered as well, since it
/// fetches, tessellates and uploads the points.
#[derive(Default)]
pub struct HeatmapPlugin;

impl<E: Environment> Plugin<E> for HeatmapPlugin {
    fn build(
        &self,
        schedule: &mut Schedule,
        _kernel: Rc<Kernel<E>>,
        world: &mut World,
        graph: &mut RenderGraph,
    ) {
        world
            .resources
            .insert(Eventually::<HeatmapResources>::Uninitialized);
        world.resources.init::<HeatmapDensityPhase>();
        schedule.add_system_to_stage(RenderStageLabel::Prepare, resource_system::resource_system);
        schedule.add_system_to_stage(RenderStageLabel::Prepare, prepare_system::prepare_system);
        schedule.add_system_to_stage(RenderStageLabel::Queue, queue_system::queue_system);

        let Some(draw_graph) = graph.get_sub_graph_mut(draw_graph::NAME) else {
            tracing::error!("draw graph is missing; heatmaps will not be drawn");
            return;
        };
        draw_graph.add_node(density_pass::DENSITY_PASS, density_pass::DensityPassNode);
        let input = draw_graph.input_node().map(|node| node.id);
        let edges = [
            input.map(|input| {
                draw_graph.add_node_edge(NodeLabel::Id(input), density_pass::DENSITY_PASS)
            }),
            Some(draw_graph.add_node_edge(density_pass::DENSITY_PASS, draw_graph::node::MAIN_PASS)),
        ];
        for edge in edges.into_iter().flatten() {
            if let Err(error) = edge {
                tracing::error!(
                    ?error,
                    "unable to order the density pass before the main pass"
                );
            }
        }
    }
}

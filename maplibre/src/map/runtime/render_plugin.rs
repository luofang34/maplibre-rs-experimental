//! Core render stages, graph labels and tile-clipping pipeline registration.

#![deny(missing_docs)]

use std::rc::Rc;

use super::{
    frame::{frame_input_system, resolve_frame_view_system},
    resource_system::ResourceSystem,
    retention_system::{evict_before_upload, retention_system},
    stage_systems::{cleanup_system, sort_phase_system, upload_system, GraphRunnerSystem},
    tile_memory::tile_bytes,
    tile_view_pattern_system::tile_view_pattern_system,
};
use crate::{
    environment::Environment,
    kernel::Kernel,
    plugin::Plugin,
    render::{
        depth_copy::{DepthCopyNode, DepthCopyPipeline},
        draw_graph,
        eventually::Eventually,
        gpu_timer,
        graph::{EmptyNode, RenderGraph, RenderGraphError},
        ground::FrameGround,
        main_pass::MainPassNode,
        projection::ProjectionGpuResources,
        render_phase::{LayerItem, RenderPhase, TileMaskItem, TranslucentItem},
        tile_mesh::{GlobeTileMeshCache, MaskPipeline},
        tile_view_pattern::{ViewTileSources, WgpuTileViewPattern},
        tracked_pass::RenderStats,
        translucent_pass::TranslucentPassNode,
        AfterSubmitCallbacks, BeforeSubmitHooks, Renderer, TileEvictor,
    },
    schedule::{
        system::{stage::SystemStage, SystemContainer},
        RenderStageLabel, Schedule,
    },
    tcs::{memory_budget::TileBytesEstimator, world::World},
};

/// Registers the frame stages, draw graph, render phases and projection resource slots.
/// Install before plugins that add systems to these stages or nodes to the draw graph.
#[derive(Default)]
pub struct RenderPlugin;

impl<E: Environment> Plugin<E> for RenderPlugin {
    fn build(
        &self,
        schedule: &mut Schedule,
        kernel: Rc<Kernel<E>>,
        world: &mut World,
        renderer: &mut Renderer,
    ) -> Result<(), crate::plugin::PluginError> {
        let resources = &mut world.resources;

        add_draw_passes(&mut renderer.render_graph)?;

        // render graph dependency
        resources.init::<RenderPhase<LayerItem>>();
        resources.init::<RenderStats>();
        resources.init::<RenderPhase<TileMaskItem>>();
        resources.init::<RenderPhase<TranslucentItem>>();
        resources.insert(TileBytesEstimator(tile_bytes));
        resources.insert(TileEvictor(evict_before_upload));
        // tile_view_pattern:
        resources.insert(Eventually::<WgpuTileViewPattern>::Uninitialized);
        resources.insert(Eventually::<ProjectionGpuResources>::Uninitialized);
        resources.init::<GlobeTileMeshCache>();
        resources.init::<ViewTileSources>();
        // masks
        resources.insert(Eventually::<MaskPipeline>::Uninitialized);
        resources.insert(Eventually::<DepthCopyPipeline>::Uninitialized);
        resources.init::<FrameGround>();
        resources
            .get_or_init_mut::<BeforeSubmitHooks>()
            .register(gpu_timer::resolve_before_submit);
        resources
            .get_or_init_mut::<AfterSubmitCallbacks>()
            .register(gpu_timer::map_after_submit);

        // The frame input comes first: the request systems the plugins add to Request must
        // already see the frame's view. `frame_input_system` is registered as early so hosts may
        // prepend `camera_flight_system` immediately after it and before `center_target_system`.
        let mut input_stage = SystemStage::default();
        input_stage
            .add_early_system(frame_input_system)
            .add_system(crate::terrain::elevation::center_target_system)
            .add_system(resolve_frame_view_system);
        schedule.add_stage(RenderStageLabel::Input, input_stage)?;
        schedule.add_stage(RenderStageLabel::Request, SystemStage::default())?;
        schedule.add_stage(
            RenderStageLabel::Prepare,
            SystemStage::default().with_system(SystemContainer::new(ResourceSystem)),
        )?;
        let queue_stage =
            SystemStage::default().with_system(crate::vector::upload_system::upload_system);
        #[cfg(feature = "raster")]
        let queue_stage = queue_stage.with_system(crate::raster::upload_system::upload_system);
        schedule.add_stage(
            RenderStageLabel::Queue,
            queue_stage
                .with_system(tile_view_pattern_system)
                .with_system(upload_system),
        )?;
        schedule.add_stage(
            RenderStageLabel::PhaseSort,
            SystemStage::default().with_system(sort_phase_system),
        )?;
        schedule.add_stage(
            RenderStageLabel::Render,
            SystemStage::default().with_system(SystemContainer::new(GraphRunnerSystem)),
        )?;
        schedule.add_stage(
            RenderStageLabel::Cleanup,
            SystemStage::default()
                .with_system(cleanup_system)
                .with_system(retention_system)
                .with_system(SystemContainer::new(StopCancelledSystem::new(&kernel))),
        )?;
        // A frame that fails part-way still clears its render phases, or the next frame
        // draws its items twice.
        schedule.run_after_failure(RenderStageLabel::Cleanup)?;
        Ok(())
    }
}

struct StopCancelledSystem<E: Environment> {
    kernel: Rc<Kernel<E>>,
}

impl<E: Environment> StopCancelledSystem<E> {
    fn new(kernel: &Rc<Kernel<E>>) -> Self {
        Self {
            kernel: kernel.clone(),
        }
    }
}

impl<E: Environment> crate::schedule::system::System for StopCancelledSystem<E> {
    fn name(&self) -> std::borrow::Cow<'static, str> {
        "stop_cancelled".into()
    }

    fn run(
        &mut self,
        crate::context::MapContext { world, .. }: &mut crate::context::MapContext,
    ) -> crate::schedule::system::SystemResult {
        crate::io::tile_retry::stop_cancelled(world, self.kernel.apc());
        crate::io::apc::AsyncProcedureCall::flush_calls(self.kernel.apc());
        Ok(())
    }
}

/// Adds the main, scene, translucent and depth-copy passes in that order.
fn add_draw_passes(graph: &mut RenderGraph) -> Result<(), RenderGraphError> {
    graph.add_node(draw_graph::node::MAIN_PASS, MainPassNode::new())?;
    graph.add_node(draw_graph::node::SCENE_3D_PASS, EmptyNode)?;
    graph.add_node(
        draw_graph::node::TRANSLUCENT_PASS,
        TranslucentPassNode::new(),
    )?;
    graph.add_node(draw_graph::node::DEPTH_COPY, DepthCopyNode)?;
    graph.add_node_edge(draw_graph::node::MAIN_PASS, draw_graph::node::SCENE_3D_PASS)?;
    graph.add_node_edge(
        draw_graph::node::SCENE_3D_PASS,
        draw_graph::node::TRANSLUCENT_PASS,
    )?;
    graph.add_node_edge(
        draw_graph::node::TRANSLUCENT_PASS,
        draw_graph::node::DEPTH_COPY,
    )
}

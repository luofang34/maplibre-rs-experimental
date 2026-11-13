//! Core render stages, graph labels and tile-clipping pipeline registration.

#![deny(missing_docs)]

use super::*;

/// Names of the render passes that draw map layers and export their depth.
pub mod draw_graph {
    /// Nodes ordered from opaque drawing through 3D scene entities and translucency to depth export.
    pub mod node {
        /// Draws tile masks and map-layer items into the frame attachments.
        pub const MAIN_PASS: &str = "main_pass";
        /// Canonical 3D scene pass slot for dynamic 3D entities (e.g. glTF aircraft and ground
        /// vehicles), ordered after `MAIN_PASS` (and `GLOBE_DAYLIGHT`) and before `OVERLAY_PASS`,
        /// `DEPTH_PROBE_PASS`, `SUN_DEPTH_COPY`, `TRANSLUCENT_PASS`, and `DEPTH_COPY`.
        pub const SCENE_3D_PASS: &str = "scene_3d_pass";
        /// Draws translucent items using the attachments populated by the main pass.
        pub const TRANSLUCENT_PASS: &str = "translucent_pass";
        /// Copies rendered depth to the optional host depth target.
        pub const DEPTH_COPY: &str = "depth_copy";
    }
}

/// GPU pipeline that writes tile identities into stencil for later clipped draws.
pub struct MaskPipeline(pub wgpu::RenderPipeline);
impl Deref for MaskPipeline {
    type Target = wgpu::RenderPipeline;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Registers the frame stages, draw graph, render phases and projection resource slots.
/// Install before plugins that add systems to these stages or nodes to the draw graph.
#[derive(Default)]
pub struct RenderPlugin;

impl<E: Environment> Plugin<E> for RenderPlugin {
    fn build(
        &self,
        schedule: &mut Schedule,
        _kernel: Rc<Kernel<E>>,
        world: &mut World,
        renderer: &mut crate::render::Renderer,
    ) -> Result<(), crate::plugin::PluginError> {
        let resources = &mut world.resources;

        add_draw_passes(&mut renderer.render_graph)?;

        // render graph dependency
        resources.init::<RenderPhase<LayerItem>>();
        resources.init::<super::tracked_pass::RenderStats>();
        resources.init::<RenderPhase<TileMaskItem>>();
        resources.init::<RenderPhase<TranslucentItem>>();
        // tile_view_pattern:
        resources.insert(Eventually::<WgpuTileViewPattern>::Uninitialized);
        resources.insert(Eventually::<projection::ProjectionGpuResources>::Uninitialized);
        resources.init::<tile_mesh::GlobeTileMeshCache>();
        resources.init::<ViewTileSources>();
        // masks
        resources.insert(Eventually::<MaskPipeline>::Uninitialized);
        resources.insert(Eventually::<DepthCopyPipeline>::Uninitialized);
        resources.init::<super::ground::FrameGround>();
        resources
            .get_or_init_mut::<super::BeforeSubmitHooks>()
            .register(super::gpu_timer::resolve_before_submit);
        resources
            .get_or_init_mut::<super::AfterSubmitCallbacks>()
            .register(super::gpu_timer::map_after_submit);

        // The frame input comes first: the request systems the plugins add to Request must
        // already see the frame's view.
        schedule.add_stage(
            RenderStageLabel::Input,
            SystemStage::default()
                .with_system(frame_input::frame_input_system)
                .with_system(crate::context::camera_flight_system)
                .with_system(crate::terrain::elevation::center_target_system)
                .with_system(resolved_view::resolve_frame_view_system),
        )?;
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
            SystemStage::default()
                .with_system(super::gpu_timer::gpu_timer_system)
                .with_system(SystemContainer::new(GraphRunnerSystem)),
        )?;
        schedule.add_stage(
            RenderStageLabel::Cleanup,
            SystemStage::default()
                .with_system(cleanup_system)
                .with_system(retention_system),
        )?;
        // A frame that fails part-way still clears its render phases, or the next frame
        // draws its items twice.
        schedule.run_after_failure(RenderStageLabel::Cleanup)?;
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

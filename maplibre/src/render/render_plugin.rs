//! Core render stages, graph labels and tile-clipping pipeline registration.

#![deny(missing_docs)]

use super::*;

/// Labels in the root graph, which drives the draw subgraph after its dependencies.
pub mod main_graph {
    /// Input-label namespace; the root graph requires no named external slots.
    pub mod input {}
    /// Root nodes used to order work before drawing.
    pub mod node {
        /// Empty dependency node that must run before the draw-subgraph driver.
        pub const MAIN_PASS_DEPENDENCIES: &str = "main_pass_dependencies";
        /// Driver that invokes the draw subgraph with the frame's render context.
        pub const MAIN_PASS_DRIVER: &str = "main_pass_driver";
    }
}

/// Labels for the subgraph that draws map layers and exports their depth.
pub mod draw_graph {
    /// Name used to register and look up this subgraph in the root graph.
    pub const NAME: &str = "draw";
    /// Input-label namespace; the draw subgraph declares no external input slots.
    pub mod input {}
    /// Nodes ordered from opaque drawing through translucency to depth export.
    pub mod node {
        /// Draws tile masks and map-layer items into the frame attachments.
        pub const MAIN_PASS: &str = "main_pass";
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
        graph: &mut RenderGraph,
    ) {
        let resources = &mut world.resources;

        let mut draw_graph = RenderGraph::default();
        // Draw nodes
        draw_graph.add_node(draw_graph::node::MAIN_PASS, MainPassNode::new());
        // Draw nodes
        draw_graph.add_node(
            draw_graph::node::TRANSLUCENT_PASS,
            TranslucentPassNode::new(),
        );
        // Input node
        let input_node_id = draw_graph.set_input(vec![]);
        // Edges
        draw_graph
            .add_node_edge(input_node_id, draw_graph::node::MAIN_PASS)
            .expect("main pass or draw node does not exist");
        draw_graph
            .add_node_edge(
                draw_graph::node::MAIN_PASS,
                draw_graph::node::TRANSLUCENT_PASS,
            )
            .expect("main pass or draw node does not exist");
        draw_graph.add_node(draw_graph::node::DEPTH_COPY, DepthCopyNode);
        draw_graph
            .add_node_edge(
                draw_graph::node::TRANSLUCENT_PASS,
                draw_graph::node::DEPTH_COPY,
            )
            .expect("translucent pass or depth copy node does not exist");

        graph.add_sub_graph(draw_graph::NAME, draw_graph);
        graph.add_node(main_graph::node::MAIN_PASS_DEPENDENCIES, EmptyNode);
        graph.add_node(main_graph::node::MAIN_PASS_DRIVER, MainPassDriverNode);
        graph
            .add_node_edge(
                main_graph::node::MAIN_PASS_DEPENDENCIES,
                main_graph::node::MAIN_PASS_DRIVER,
            )
            .expect("main pass driver or dependencies do not exist");

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

        // The frame input comes first: the request systems the plugins add to Extract must
        // already see the frame's view.
        schedule.add_stage(
            RenderStageLabel::Extract,
            SystemStage::default()
                .with_system(frame_input::frame_input_system)
                .with_system(crate::terrain::elevation::center_target_system),
        );
        schedule.add_stage(
            RenderStageLabel::Prepare,
            SystemStage::default().with_system(SystemContainer::new(ResourceSystem)),
        );
        schedule.add_stage(
            RenderStageLabel::Queue,
            SystemStage::default()
                .with_system(tile_view_pattern_system)
                .with_system(upload_system),
        );
        schedule.add_stage(
            RenderStageLabel::PhaseSort,
            SystemStage::default().with_system(sort_phase_system),
        );
        schedule.add_stage(
            RenderStageLabel::Render,
            SystemStage::default().with_system(SystemContainer::new(GraphRunnerSystem)),
        );
        schedule.add_stage(
            RenderStageLabel::Cleanup,
            SystemStage::default()
                .with_system(cleanup_system)
                .with_system(retention_system),
        );
    }
}

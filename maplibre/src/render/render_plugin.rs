use super::*;

pub mod main_graph {
    // Labels for input nodes
    pub mod input {}
    // Labels for non-input nodes
    pub mod node {
        pub const MAIN_PASS_DEPENDENCIES: &str = "main_pass_dependencies";
        pub const MAIN_PASS_DRIVER: &str = "main_pass_driver";
    }
}

/// Labels for the "draw" graph
pub mod draw_graph {
    pub const NAME: &str = "draw";
    // Labels for input nodes
    pub mod input {}
    // Labels for non-input nodes
    pub mod node {
        pub const MAIN_PASS: &str = "main_pass";
        pub const TRANSLUCENT_PASS: &str = "translucent_pass";
        pub const DEPTH_COPY: &str = "depth_copy";
    }
}

pub struct MaskPipeline(pub wgpu::RenderPipeline);
impl Deref for MaskPipeline {
    type Target = wgpu::RenderPipeline;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

// TODO: Do we really want a render plugin or do we want to statically do this setup?
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
            SystemStage::default().with_system(frame_input::frame_input_system),
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

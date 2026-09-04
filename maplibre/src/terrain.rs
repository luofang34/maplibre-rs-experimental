//! Terrain: elevation tiles from `raster-dem` sources, draped rendering, and elevation queries.

use std::{collections::HashSet, marker::PhantomData, rc::Rc};

use crate::{
    coords::WorldTileCoords,
    environment::Environment,
    kernel::Kernel,
    plugin::Plugin,
    render::{
        draw_graph,
        eventually::Eventually,
        graph::{NodeLabel, RenderGraph},
        render_phase::{LayerItem, TileMaskItem},
        RenderStageLabel,
    },
    schedule::Schedule,
    tcs::{system::SystemContainer, tiles::TileComponent, world::World},
};

pub mod backfill;
pub mod coverage;
pub mod dem;
mod drape_pass;
mod draw;
pub mod elevation;
pub mod mesh;
mod populate_world_system;
mod queue_system;
mod request_system;
mod resource_system;
pub mod resources;
pub mod rtt;
pub mod source;
mod transferables;
mod upload_system;

pub use backfill::backfill_neighbours;
pub use coverage::{TerrainCoverageIndex, TerrainSample};
pub use dem::{DemError, DemTile};
use drape_pass::{DrapePassNode, DRAPE_PASS};
pub use draw::draw_terrain;
pub use elevation::elevation_at_world;
use populate_world_system::PopulateWorldSystem;
pub use queue_system::is_drapeable;
use request_system::RequestSystem;
pub use request_system::{dem_ancestor_coords, dem_tile_coords, fetch_dem_apc};
use resources::TerrainResources;
pub use transferables::{
    DefaultDemTransferables, DefaultLayerDem, DefaultLayerDemMissing, DemMessageTag,
    DemTransferables, LayerDem, LayerDemMissing,
};

/// A decoded DEM tile together with its neighbour bookkeeping.
#[derive(Debug)]
pub struct LoadedDem {
    /// Decoded samples.
    pub tile: DemTile,
    /// Neighbours whose edge samples already replaced this tile's replicated border.
    pub backfilled: HashSet<WorldTileCoords>,
    /// Advances whenever the samples change, so the GPU copy can follow.
    pub revision: u32,
}

impl LoadedDem {
    /// Wraps a freshly decoded tile with no neighbours filled in yet.
    pub fn new(tile: DemTile) -> Self {
        Self {
            tile,
            backfilled: HashSet::new(),
            revision: 0,
        }
    }
}

/// Elevation data of one `raster-dem` tile as it moves through the pipeline.
///
/// Stored at the DEM tile's own coordinates, which sit one zoom level below the tiles they drape.
#[derive(Debug)]
pub enum DemTileComponent {
    /// The tile has been requested and is being fetched or decoded.
    Pending,
    /// The tile is available for rendering and elevation queries.
    Loaded(LoadedDem),
    /// The tile could not be fetched or decoded; ancestors stand in for it.
    Missing,
}

impl TileComponent for DemTileComponent {}

/// Layers of one view tile rendered into that tile's drape texture.
pub struct DrapeTarget {
    /// View tile the texture belongs to.
    pub coords: WorldTileCoords,
    /// Color the texture is cleared to before drawing.
    pub clear_color: wgpu::Color,
    /// Stencil masks of the source shapes.
    pub masks: Vec<TileMaskItem>,
    /// Drapeable layer draws in style order.
    pub layers: Vec<LayerItem>,
}

/// Drape targets of the current frame.
#[derive(Default)]
pub struct DrapePhase {
    /// One entry per view tile with drape content.
    pub targets: Vec<DrapeTarget>,
}

/// Where the main pass draws the terrain this frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct TerrainFrame {
    /// Whether terrain tiles are queued at all.
    pub active: bool,
    /// Layer index after which the terrain is drawn, so backgrounds stay beneath it.
    pub draw_after_layer_index: u32,
}

/// Requests, stores, drapes and draws the DEM tiles of a style that declares `terrain`.
pub struct TerrainPlugin<T>(PhantomData<T>);

impl<T: DemTransferables> Default for TerrainPlugin<T> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

impl<E: Environment, T: DemTransferables> Plugin<E> for TerrainPlugin<T> {
    fn build(
        &self,
        schedule: &mut Schedule,
        kernel: Rc<Kernel<E>>,
        world: &mut World,
        graph: &mut RenderGraph,
    ) {
        world
            .resources
            .insert(Eventually::<TerrainResources>::Uninitialized);
        world.resources.init::<DrapePhase>();
        world.resources.init::<TerrainFrame>();
        world.resources.init::<TerrainCoverageIndex>();

        schedule.add_system_to_stage(
            RenderStageLabel::Extract,
            SystemContainer::new(RequestSystem::<E, T>::new(&kernel)),
        );
        schedule.add_system_to_stage(
            RenderStageLabel::Extract,
            SystemContainer::new(PopulateWorldSystem::<E, T>::new(&kernel)),
        );
        // Prepare rather than Extract: headless rendering drops the Extract stage, and the
        // elevation must be known before the Queue stage builds the view pattern. The index
        // runs first so the center elevation samples this frame's tiles.
        schedule.add_system_to_stage(RenderStageLabel::Prepare, coverage::coverage_system);
        schedule.add_system_to_stage(
            RenderStageLabel::Prepare,
            elevation::center_elevation_system,
        );
        schedule.add_system_to_stage(RenderStageLabel::Prepare, resource_system::resource_system);
        schedule.add_system_to_stage(RenderStageLabel::Queue, upload_system::upload_system);
        schedule.add_system_to_stage(RenderStageLabel::Queue, queue_system::queue_system);

        let Some(draw_graph) = graph.get_sub_graph_mut(draw_graph::NAME) else {
            tracing::error!("draw graph is missing; terrain will not be drawn");
            return;
        };
        draw_graph.add_node(DRAPE_PASS, DrapePassNode);
        let input = draw_graph.input_node().map(|node| node.id);
        let edges = [
            input.map(|input| draw_graph.add_node_edge(NodeLabel::Id(input), DRAPE_PASS)),
            Some(draw_graph.add_node_edge(DRAPE_PASS, draw_graph::node::MAIN_PASS)),
        ];
        for edge in edges.into_iter().flatten() {
            if let Err(error) = edge {
                tracing::error!(
                    ?error,
                    "unable to order the drape pass before the main pass"
                );
            }
        }
    }
}

//! Terrain: elevation tiles from `raster-dem` sources, draped rendering, and elevation queries.

use std::{marker::PhantomData, rc::Rc};

use crate::{
    coords::WorldTileCoords,
    environment::Environment,
    kernel::Kernel,
    plugin::Plugin,
    render::{
        draw_graph,
        eventually::Eventually,
        render_phase::{LayerItem, TileMaskItem},
        RenderStageLabel,
    },
    schedule::Schedule,
    tcs::{system::SystemContainer, tiles::TileComponent, world::World},
};

pub mod backfill;
pub mod coverage;
pub mod dem;
pub mod dem_store;
mod drape_cache;
mod drape_pass;
pub(crate) mod drape_targets;
pub mod drape_timing;
mod draw;
pub mod elevation;
pub mod gpu_dem_pool;
pub mod interaction;
mod loaded_dem;
pub mod mesh;
pub(crate) mod populate_world_system;
pub(crate) mod queue_system;
pub(crate) mod request_system;
mod resource_system;
pub mod resources;
pub mod rtt;
pub mod sightline;
pub mod source;
pub mod transferables;
mod upload_system;

pub use backfill::backfill_neighbours;

/// Whether terrain tiles hang skirts below their edges, as GL JS `terrainSkirtLength`: skirts
/// hide the cracks between neighbouring tiles of different levels, and show through a
/// translucent drape.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TerrainSkirts {
    /// Skirts drop by a fifth of a tile's circumference at the camera's zoom.
    #[default]
    Auto,
    /// No skirts.
    None,
}

/// Elevation in metres above datum at the geographic north (`90° N`) and south (`90° S`)
/// poles, closing the globe beyond Web Mercator DEM coverage (`|lat| <= 85.0511°`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PolarPoleElevations {
    /// Elevation in metres at `90° N` (`0.0` by default).
    pub north_meters: f32,
    /// Elevation in metres at `90° S` (`0.0` by default).
    pub south_meters: f32,
}
pub use coverage::{DemDetail, TerrainCoverageIndex};
pub use dem::{DemError, DemTile};
pub use dem_store::DemLayersComponent;
use drape_pass::{DrapePassNode, DRAPE_PASS};
pub use drape_targets::is_drapeable;
pub use draw::draw_terrain;
pub use gpu_dem_pool::GpuDemPool;
pub use interaction::{GestureAnchor, TerrainHit};
pub(crate) use loaded_dem::DemRevision;
pub use loaded_dem::LoadedDem;
use populate_world_system::PopulateWorldSystem;
pub(crate) use queue_system::surface_covering::drawn_surface_tiles;
pub use queue_system::DrapeBudget;
use request_system::RequestSystem;
pub use request_system::{dem_ancestor_coords, dem_tile_coords, fetch_dem_apc};
use resources::TerrainResources;
pub use transferables::{
    DefaultDemTransferables, DefaultLayerDem, DefaultLayerDemMissing, DemMessageTag,
    DemTransferables, LayerDem, LayerDemMissing,
};

/// Elevation data of one `raster-dem` tile as it moves through the pipeline.
///
/// Stored at the DEM tile's own coordinates, which sit one zoom level below the tiles they drape.
#[derive(Clone, Debug)]
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
    /// One entry per view tile whose texture is redrawn this frame.
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
        renderer: &mut crate::render::Renderer,
    ) -> Result<(), crate::plugin::PluginError> {
        world
            .resources
            .insert(Eventually::<TerrainResources>::Uninitialized);
        world.resources.init::<DrapePhase>();
        world.resources.init::<TerrainFrame>();
        world.resources.init::<TerrainCoverageIndex>();

        schedule.add_system_to_stage(
            RenderStageLabel::Request,
            SystemContainer::new(RequestSystem::<E, T>::new(&kernel)),
        )?;
        schedule.add_system_to_stage(
            RenderStageLabel::Request,
            SystemContainer::new(PopulateWorldSystem::<E, T>::new(&kernel)),
        )?;
        // Prepare rather than Request: headless rendering drops the Request stage, and the
        // elevation must be known before the Queue stage builds the view pattern. The index
        // runs first so the center elevation samples this frame's tiles.
        schedule.add_system_to_stage(RenderStageLabel::Prepare, coverage::coverage_system)?;
        schedule.add_system_to_stage(
            RenderStageLabel::Prepare,
            elevation::center_elevation_system,
        )?;
        schedule.add_system_to_stage(
            RenderStageLabel::Prepare,
            crate::render::resolved_view::resolve_frame_view_system,
        )?;
        schedule
            .add_system_to_stage(RenderStageLabel::Prepare, resource_system::resource_system)?;
        schedule.add_system_to_stage(RenderStageLabel::Queue, upload_system::upload_system)?;
        schedule.add_system_to_stage(RenderStageLabel::Queue, queue_system::queue_system)?;
        // Before the Render stage, so the last measurement is read before this frame's drape
        // passes start the next one.
        schedule.add_system_to_stage(
            RenderStageLabel::PhaseSort,
            drape_timing::drape_timer_system,
        )?;
        world
            .resources
            .get_or_init_mut::<crate::render::BeforeSubmitHooks>()
            .register(drape_timing::resolve_before_submit);
        world
            .resources
            .get_or_init_mut::<crate::render::AfterSubmitCallbacks>()
            .register(drape_timing::map_after_submit);

        let draw_graph = &mut renderer.render_graph;
        let edges = [
            draw_graph.add_node(DRAPE_PASS, DrapePassNode),
            draw_graph.add_node_edge(DRAPE_PASS, draw_graph::node::MAIN_PASS),
        ];
        for edge in edges {
            edge?;
        }
        Ok(())
    }
}

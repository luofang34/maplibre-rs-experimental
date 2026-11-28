//! Terrain: elevation tiles from `raster-dem` sources, draped rendering, and elevation queries.

use crate::{
    coords::WorldTileCoords,
    render::render_phase::{LayerItem, TileMaskItem},
};

mod data;
mod drape;
mod surface;
mod systems;

pub use backfill::backfill_neighbours;
pub(crate) use data::loaded_dem::DemRevision;
pub use data::{
    backfill, dem,
    dem::{dem_tile_coords, DemError, DemTile},
    dem_store,
    dem_store::DemLayersComponent,
    loaded_dem::LoadedDem,
    mesh, source, transferables,
    transferables::{
        DefaultDemTransferables, DefaultLayerDem, DefaultLayerDemMissing, DemMessageTag,
        DemTransferables, LayerDem, LayerDemMissing,
    },
};
pub(crate) use drape::{
    drape_cache,
    drape_pass::{DrapePassNode, DRAPE_PASS},
    drape_share, drape_targets,
};
pub use drape::{
    drape_targets::is_drapeable, drape_timing, draw::draw_terrain, gpu_dem_pool,
    gpu_dem_pool::GpuDemPool, resources, rtt,
};
pub use surface::{
    coverage,
    coverage::{DemDetail, TerrainCoverageIndex},
    elevation, interaction,
    interaction::{GestureAnchor, TerrainHit},
    sightline,
};
pub(crate) use systems::{
    populate_world_system, queue_system, request_system, resource_system, upload_system,
};
pub use systems::{
    queue_system::DrapeBudget,
    request_system::{dem_ancestor_coords, fetch_dem_apc},
};

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
#[cfg(all(test, feature = "headless"))]
pub(crate) use surface::coverage::drawn_surface_tiles;

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

impl crate::tcs::tiles::TileComponent for DemTileComponent {}

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

/// Populates terrain-specific pass frame slots once per render pass.
pub(crate) fn populate_terrain_pass_refs<'w>(
    world: &'w crate::tcs::world::World,
    refs: &mut crate::render::pass_refs::PassFrameRefs<'w>,
) {
    use crate::render::pass_refs::initialized;
    let active = world
        .resources
        .get::<TerrainFrame>()
        .is_some_and(|frame| frame.active);
    refs.set_terrain(active, initialized::<resources::TerrainResources>(world));
}

pub(crate) use crate::tcs::tile_epochs::{bump_dem_arrival_epoch, dem_arrival_epoch};

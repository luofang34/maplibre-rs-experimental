//! Terrain: elevation tiles from `raster-dem` sources, draped rendering, and elevation queries.

use std::{marker::PhantomData, rc::Rc};

use crate::{
    environment::Environment,
    kernel::Kernel,
    plugin::Plugin,
    render::{graph::RenderGraph, RenderStageLabel},
    schedule::Schedule,
    tcs::{system::SystemContainer, tiles::TileComponent, world::World},
};

pub mod dem;
mod populate_world_system;
mod request_system;
pub mod source;
mod transferables;

pub use dem::{DemError, DemTile};
use populate_world_system::PopulateWorldSystem;
use request_system::RequestSystem;
pub use request_system::{dem_tile_coords, fetch_dem_apc};
pub use transferables::{
    DefaultDemTransferables, DefaultLayerDem, DefaultLayerDemMissing, DemMessageTag,
    DemTransferables, LayerDem, LayerDemMissing,
};

/// Elevation data of one `raster-dem` tile as it moves through the pipeline.
///
/// Stored at the DEM tile's own coordinates, which sit one zoom level below the tiles they drape.
#[derive(Debug)]
pub enum DemTileComponent {
    /// The tile has been requested and is being fetched or decoded.
    Pending,
    /// The tile is available for rendering and elevation queries.
    Loaded(DemTile),
    /// The tile could not be fetched or decoded; ancestors stand in for it.
    Missing,
}

impl TileComponent for DemTileComponent {}

/// Requests and stores the DEM tiles of a style that declares `terrain`.
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
        _world: &mut World,
        _graph: &mut RenderGraph,
    ) {
        schedule.add_system_to_stage(
            RenderStageLabel::Extract,
            SystemContainer::new(RequestSystem::<E, T>::new(&kernel)),
        );
        schedule.add_system_to_stage(
            RenderStageLabel::Extract,
            SystemContainer::new(PopulateWorldSystem::<E, T>::new(&kernel)),
        );
    }
}

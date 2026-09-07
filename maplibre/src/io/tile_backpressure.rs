//! Bounds how many tiles are being fetched and processed at once.
//!
//! A covering can ask for hundreds of tiles in one frame: a flight's destination joins the
//! requests, and a warm disk cache answers every one of them within a second. Each worker
//! then tessellates a tile at a time, and a low-zoom planet tile takes hundreds of
//! megabytes while it is being built. Processing dozens at once exhausts a headset's memory
//! limit. The request systems take the nearest tiles first and stop once this many are in
//! flight, so a burst becomes a stream the frame loop drains.

use crate::{
    coords::WorldTileCoords,
    raster::RasterLayersDataComponent,
    render::memory_budget::MemoryBudget,
    tcs::{tiles::Tiles, world::World},
    terrain::DemTileComponent,
    vector::VectorLayerBucketComponent,
};

/// Tiles that may be fetched or processed at the same time across every source.
pub const MAX_TILES_IN_FLIGHT: usize = 24;

/// Whether every request for the tile has produced a result.
pub(crate) fn is_settled(tiles: &Tiles, coords: WorldTileCoords) -> bool {
    let vector_loading = tiles
        .query::<&VectorLayerBucketComponent>(coords)
        .is_some_and(|component| !component.done);
    let raster_loading = tiles
        .query::<&RasterLayersDataComponent>(coords)
        .is_some_and(|component| component.layers.is_empty());
    let dem_loading = matches!(
        tiles.query::<&DemTileComponent>(coords),
        Some(DemTileComponent::Pending)
    );
    !(vector_loading || raster_loading || dem_loading)
}

/// Tiles with a request that has not produced its result yet.
pub fn tiles_in_flight(tiles: &Tiles) -> usize {
    tiles
        .tiles
        .values()
        .filter(|tile| !is_settled(tiles, tile.coords))
        .count()
}

/// How many more tiles may be requested this frame: up to the in-flight bound, fewer
/// while the host is short of memory and none while it is nearly out.
pub fn request_budget(world: &World) -> usize {
    let allowed = world
        .resources
        .get::<MemoryBudget>()
        .copied()
        .unwrap_or_default()
        .tiles_in_flight_allowed(MAX_TILES_IN_FLIGHT);
    allowed.saturating_sub(tiles_in_flight(&world.tiles))
}

#[cfg(test)]
mod tests;

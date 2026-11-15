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
    io::tile_load,
    render::memory_budget::MemoryBudget,
    tcs::{tiles::Tiles, world::World},
};

/// Tiles that may be fetched or processed at the same time across every source.
pub const MAX_TILES_IN_FLIGHT: usize = 16;

/// Records whether each request family had to skip at least one wanted tile in its last run
/// because the in-flight request budget was exhausted (`budget == 0`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct BackpressureDeferred {
    pub vector: bool,
    pub raster: bool,
    pub dem: bool,
}

impl BackpressureDeferred {
    fn any(self) -> bool {
        self.vector || self.raster || self.dem
    }
}

/// Updates whether the vector request pass deferred wanted tiles under backpressure.
pub(crate) fn set_vector_deferred(world: &mut World, deferred: bool) {
    world
        .resources
        .get_or_init_mut::<BackpressureDeferred>()
        .vector = deferred;
}

/// Updates whether the raster request pass deferred wanted tiles under backpressure.
#[cfg(any(test, feature = "raster"))]
pub(crate) fn set_raster_deferred(world: &mut World, deferred: bool) {
    world
        .resources
        .get_or_init_mut::<BackpressureDeferred>()
        .raster = deferred;
}

/// Updates whether the DEM request pass deferred wanted tiles under backpressure.
pub(crate) fn set_dem_deferred(world: &mut World, deferred: bool) {
    world
        .resources
        .get_or_init_mut::<BackpressureDeferred>()
        .dem = deferred;
}

/// Whether wanted tiles remain unrequested while the host allows in-flight tile work.
pub(crate) fn has_unrequested_visible_tiles(world: &World) -> bool {
    world
        .resources
        .get::<BackpressureDeferred>()
        .is_some_and(|deferred| deferred.any())
        && allowed_in_flight(world) > 0
}

/// Whether deferred wanted tiles can be dispatched immediately on the next frame because
/// `tiles_in_flight` has dropped below the allowed in-flight ceiling.
pub(crate) fn has_ready_deferred_requests(world: &World) -> bool {
    has_unrequested_visible_tiles(world) && request_budget(world) > 0
}

/// Whether every request for the tile has produced its outcome or was given up.
pub(crate) fn is_settled(tiles: &Tiles, coords: WorldTileCoords) -> bool {
    !tile_load::is_loading(tiles, coords)
}

/// Tiles with a request that has not produced its outcome yet.
pub fn tiles_in_flight(world: &World) -> usize {
    tile_load::loading_count(world)
}

/// Cumulative count of tile requests admitted across all request families in `world`.
pub fn total_tile_requests(world: &World) -> u64 {
    tile_load::total_issued(world)
}

fn allowed_in_flight(world: &World) -> usize {
    world
        .resources
        .get::<MemoryBudget>()
        .copied()
        .unwrap_or_default()
        .tiles_in_flight_allowed(MAX_TILES_IN_FLIGHT)
}

/// How many more tiles may be requested this frame: up to the in-flight bound, fewer
/// while the host is short of memory and none while it is nearly out.
pub fn request_budget(world: &World) -> usize {
    allowed_in_flight(world).saturating_sub(tiles_in_flight(world))
}

/// Shares available requests with the DEM pass without starving vector tiles under pressure.
pub(crate) fn vector_request_budget(world: &World, has_terrain: bool) -> usize {
    let available = request_budget(world);
    let reserved = if has_terrain {
        (available / 2).min(4)
    } else {
        0
    };
    available - reserved
}

/// Shares available requests with the raster pass so slow remote DEM fetches never starve
/// local or draped raster tiles.
pub(crate) fn dem_request_budget(world: &World, has_raster: bool) -> usize {
    let available = request_budget(world);
    let reserved = if has_raster {
        (available / 3).min(8)
    } else {
        0
    };
    available - reserved
}

#[cfg(test)]
mod tests;

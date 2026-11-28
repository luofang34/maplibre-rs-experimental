//! Monotonic per-subsystem epoch counters tracking tile arrival and GPU upload stamps.

use crate::tcs::world::World;

fn next_epoch(current: u64) -> u64 {
    current.wrapping_add(1)
}

/// Monotonic epoch bumped whenever vector tile worker results commit or tiles evict.
#[derive(Default)]
pub(crate) struct VectorArrivalEpoch(pub u64);

/// Bumps the vector arrival epoch after committing worker results or evicting vector tiles.
pub(crate) fn bump_vector_arrival_epoch(world: &mut World) {
    let epoch = world.resources.get_or_init_mut::<VectorArrivalEpoch>();
    epoch.0 = next_epoch(epoch.0);
}

/// Returns the current vector arrival epoch.
pub(crate) fn vector_arrival_epoch(world: &World) -> u64 {
    world
        .resources
        .get::<VectorArrivalEpoch>()
        .map_or(0, |epoch| epoch.0)
}

/// Monotonic epoch bumped whenever any tile's vector `uploaded_stamp` changes.
#[derive(Default)]
pub(crate) struct VectorUploadStampEpoch(pub u64);

/// Bumps the vector upload-stamp epoch after modifying a tile's upload stamp.
pub(crate) fn bump_vector_upload_stamp_epoch(world: &mut World) {
    let epoch = world.resources.get_or_init_mut::<VectorUploadStampEpoch>();
    epoch.0 = next_epoch(epoch.0);
}

/// Returns the current vector upload-stamp epoch.
pub(crate) fn vector_upload_stamp_epoch(world: &World) -> u64 {
    world
        .resources
        .get::<VectorUploadStampEpoch>()
        .map_or(0, |epoch| epoch.0)
}

/// Monotonic epoch bumped whenever raster or shared DEM layer tile data commits or evicts.
#[derive(Default)]
pub(crate) struct RasterArrivalEpoch(pub u64);

/// Bumps the raster arrival epoch after committing raster results or evicting tiles.
pub(crate) fn bump_raster_arrival_epoch(world: &mut World) {
    let epoch = world.resources.get_or_init_mut::<RasterArrivalEpoch>();
    epoch.0 = next_epoch(epoch.0);
}

/// Returns the current raster arrival epoch.
pub(crate) fn raster_arrival_epoch(world: &World) -> u64 {
    world
        .resources
        .get::<RasterArrivalEpoch>()
        .map_or(0, |epoch| epoch.0)
}

/// Monotonic epoch bumped whenever terrain DEM tile state commits, ingests, or evicts.
#[derive(Default)]
pub(crate) struct DemArrivalEpoch(pub u64);

/// Bumps the DEM arrival epoch after committing terrain DEM results or evicting tiles.
pub(crate) fn bump_dem_arrival_epoch(world: &mut World) {
    let epoch = world.resources.get_or_init_mut::<DemArrivalEpoch>();
    epoch.0 = next_epoch(epoch.0);
}

/// Returns the current DEM arrival epoch.
pub(crate) fn dem_arrival_epoch(world: &World) -> u64 {
    world
        .resources
        .get::<DemArrivalEpoch>()
        .map_or(0, |epoch| epoch.0)
}

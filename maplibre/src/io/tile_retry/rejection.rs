//! Requests the worker transport refused to take, which wait out a backoff like failed ones
//! instead of being offered again every frame.

use super::{mark_refresh_sources, now, observed_now, RequestKind, TileRequestRetries};
use crate::{coords::WorldTileCoords, tcs::world::World};

/// Records that the transport refused the request for the tile, so it is offered again once
/// a backoff passes; a host drawing on demand wakes for it after
/// [`crate::context::MapContext::next_retry_delay`]. `issued_for` is what the request was
/// for, so the retry is for it again.
pub(crate) fn rejected(
    world: &mut World,
    coords: WorldTileCoords,
    kind: RequestKind,
    issued_for: Option<&[String]>,
) {
    let now = now(world);
    if world.tiles.query::<&TileRequestRetries>(coords).is_none() {
        if let Some(mut tile) = world.tiles.spawn_mut(coords) {
            tile.insert(TileRequestRetries::default());
        }
    }
    let Some(retries) = world.tiles.query_mut::<&mut TileRequestRetries>(coords) else {
        return;
    };
    let state = &mut retries.0[kind.index()];
    state.pending = false;
    state.stale = false;
    state.back_off(now);
    mark_refresh_sources(world, coords, issued_for);
}

/// Whether the tile waits out a backoff before its next request, with none in flight.
pub(crate) fn backing_off(world: &World, coords: WorldTileCoords, kind: RequestKind) -> bool {
    let now = observed_now(world);
    world
        .tiles
        .query::<&TileRequestRetries>(coords)
        .is_some_and(|retries| {
            let state = &retries.0[kind.index()];
            !state.pending && state.deadline.is_some_and(|due| due > now)
        })
}

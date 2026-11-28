//! Completion and bounded retry scheduling for vector, raster and elevation requests.

use std::{cell::Cell, collections::BTreeSet, rc::Rc, time::Duration};

use serde::{Deserialize, Serialize};
pub use state::RequestDisposition;
use state::TileRequestRetries;

pub use super::tile_load::RequestKind;
use crate::{
    coords::WorldTileCoords,
    io::apc::{IntoMessage, Message},
    tcs::{tiles::TileComponent, world::World},
};

/// Final worker result; its attempt identifies the admitted request across tile eviction.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct TileRequestOutcome {
    /// Tile coordinates shared by this request's source results.
    pub coords: WorldTileCoords,
    /// Request family whose admission this outcome finishes.
    pub kind: RequestKind,
    /// Map-owned attempt identifier; direct untracked calls carry `None`.
    pub attempt: Option<u64>,
    /// Whether the request needs another attempt.
    pub disposition: RequestDisposition,
}

impl IntoMessage for TileRequestOutcome {
    fn into(self) -> Message {
        Message::new(self.kind.message_tag(), Box::new(self))
    }
}

// Workers retain the map transport while reset replaces the world and its deadlines.
#[derive(Clone, Default)]
pub(crate) struct RequestAttempts(Rc<Cell<u64>>);

impl RequestAttempts {
    pub(crate) fn next(&self) -> u64 {
        let next = self.0.get().wrapping_add(1);
        self.0.set(next);
        next
    }
}

/// Whether a frame has request work: a request in flight, whose result the next frame applies,
/// or a retry whose deadline has passed, which the next frame issues.
pub(crate) fn needs_frame(world: &World) -> bool {
    index::needs_frame_indexed(world)
}

/// Shortest wait until a scheduled retry deadline is due, or `None` when no retry waits.
pub(crate) fn next_retry_delay(world: &World) -> Option<Duration> {
    index::next_retry_delay_indexed(world)
}

/// Whether any tile of `kind` has a retry deadline that is already due.
pub(crate) fn kind_has_due_retry(world: &World, kind: RequestKind) -> bool {
    index::kind_has_due_retry_indexed(world, kind)
}

/// Requests every requested tile of `kind` again, keeping what it shows until the new data lands.
pub(crate) fn refresh(world: &mut World, kind: RequestKind) {
    let coords: Vec<WorldTileCoords> = world
        .tiles
        .pyramid_of::<TileRequestRetries>()
        .coords_coarse_to_fine()
        .collect();
    refresh_tiles(world, kind, &coords);
}

/// What the pending refresh of a tile is for, so that the request made for it can leave the
/// tile's other sources as they are.
#[derive(Default)]
enum RefreshSources {
    /// No refresh is pending, or the request for it has been made.
    #[default]
    None,
    /// The data of these sources changed.
    Some(BTreeSet<String>),
    /// Anything about the tile may have changed.
    Every,
}
impl TileComponent for RefreshSources {}

/// Like [`refresh_tiles`], for a change to the data of `sources` alone: a request made for
/// the refresh processes those sources and keeps what the tile shows of the others.
#[cfg(any(test, feature = "geojson"))]
pub(crate) fn refresh_tiles_for_sources(
    world: &mut World,
    kind: RequestKind,
    tiles: &[WorldTileCoords],
    sources: &[String],
) {
    for coords in tiles {
        mark_refresh_sources(world, *coords, Some(sources));
    }
    refresh_due(world, kind, tiles);
}

/// Notes which sources a pending refresh of the tile is for: those given add to the ones
/// noted before, and a refresh for every source (`None`) outranks any for some, whichever
/// comes first.
fn mark_refresh_sources(world: &mut World, coords: WorldTileCoords, sources: Option<&[String]>) {
    let marked = match sources {
        None => RefreshSources::Every,
        Some(sources) => RefreshSources::Some(sources.iter().cloned().collect()),
    };
    if let Some(existing) = world.tiles.query_mut::<&mut RefreshSources>(coords) {
        match (existing, marked) {
            (RefreshSources::Every, _) => {}
            (RefreshSources::Some(scope), RefreshSources::Some(sources)) => scope.extend(sources),
            (existing, marked) => *existing = marked,
        }
        return;
    }
    if let Some(mut tile) = world.tiles.spawn_mut(coords) {
        tile.insert(marked);
    }
}

/// The sources the pending refresh of the tile is for, cleared for the request that takes
/// them; `None` when the refresh is for every source, or none is pending.
pub(crate) fn take_refresh_sources(
    world: &mut World,
    coords: WorldTileCoords,
) -> Option<Vec<String>> {
    let pending = world.tiles.query_mut::<&mut RefreshSources>(coords)?;
    match std::mem::take(pending) {
        RefreshSources::Some(sources) => Some(sources.into_iter().collect()),
        RefreshSources::None | RefreshSources::Every => None,
    }
}

/// Requests `tiles` again for `kind`, keeping what they show until the new data lands.
pub(crate) fn refresh_tiles(world: &mut World, kind: RequestKind, tiles: &[WorldTileCoords]) {
    for coords in tiles {
        mark_refresh_sources(world, *coords, None);
    }
    refresh_due(world, kind, tiles);
}

/// Makes the next request for each of `tiles` due now, or once the one in flight lands.
fn refresh_due(world: &mut World, kind: RequestKind, tiles: &[WorldTileCoords]) {
    crate::io::frame_signals::mark_dirty(world);
    let now = now(world);
    let mut newly_scheduled = Vec::new();
    for coords in tiles {
        let out_of_view = world
            .resources
            .get::<index::RetryIndex>()
            .is_some_and(|idx| idx.is_out_of_view(kind, *coords));
        let Some(retries) = world.tiles.query_mut::<&mut TileRequestRetries>(*coords) else {
            continue;
        };
        if retries.slot_mut(kind).schedule_refresh(now, out_of_view) {
            newly_scheduled.push(*coords);
        }
    }
    if !newly_scheduled.is_empty() {
        let idx = world.resources.get_or_init_mut::<index::RetryIndex>();
        for coords in newly_scheduled {
            idx.mark_scheduled(kind, coords);
        }
    }
}

/// Requests a finished tile again after `delay`, because part of what its worker sent later
/// was temporarily unavailable.
pub(crate) fn retry_later(
    world: &mut World,
    coords: WorldTileCoords,
    kind: RequestKind,
    delay: Duration,
) {
    let now = now(world);
    let out_of_view = world
        .resources
        .get::<index::RetryIndex>()
        .is_some_and(|idx| idx.is_out_of_view(kind, coords));
    let Some(retries) = world.tiles.query_mut::<&mut TileRequestRetries>(coords) else {
        return;
    };
    if !retries
        .slot_mut(kind)
        .schedule_after(now, delay, out_of_view)
    {
        return;
    }
    world
        .resources
        .get_or_init_mut::<index::RetryIndex>()
        .mark_scheduled(kind, coords);
    // The retry brings what the whole tile was asked for, not what a later data change was.
    mark_refresh_sources(world, coords, None);
}

pub(crate) fn due(world: &mut World, coords: WorldTileCoords, kind: RequestKind) -> bool {
    let now = now(world);
    world
        .tiles
        .query::<&TileRequestRetries>(coords)
        .is_some_and(|retries| retries.slot(kind).is_due(now))
}

pub(crate) fn is_pending(world: &World, coords: WorldTileCoords, kind: RequestKind) -> bool {
    world
        .tiles
        .query::<&TileRequestRetries>(coords)
        .is_some_and(|retries| retries.slot(kind).is_pending())
}

#[cfg(all(test, feature = "headless", not(target_arch = "wasm32")))]
pub(crate) fn pending_attempt(
    world: &World,
    coords: WorldTileCoords,
    kind: RequestKind,
) -> Option<u64> {
    world
        .tiles
        .query::<&TileRequestRetries>(coords)
        .and_then(|retries| retries.slot(kind).pending_attempt())
}

#[cfg(any(test, feature = "raster"))]
pub(crate) fn waiting(world: &World, coords: WorldTileCoords, kind: RequestKind) -> bool {
    world
        .tiles
        .query::<&TileRequestRetries>(coords)
        .is_some_and(|retries| retries.slot(kind).is_waiting())
}

#[cfg(feature = "geojson")]
pub(crate) fn is_waiting_or_pending(
    tiles: &crate::tcs::tiles::Tiles,
    coords: WorldTileCoords,
    kind: RequestKind,
) -> bool {
    tiles
        .query::<&TileRequestRetries>(coords)
        .is_some_and(|retries| retries.slot(kind).is_waiting())
}

pub(crate) fn next_attempt(world: &mut World) -> u64 {
    let attempts = world.resources.get_or_init_mut::<RequestAttempts>();
    let next = attempts.0.get().wrapping_add(1);
    attempts.0.set(next);
    next
}

/// Records the attempt made for the tile, limited to `only_sources` or for every source.
pub(crate) fn started(
    world: &mut World,
    coords: WorldTileCoords,
    kind: RequestKind,
    attempt: u64,
    only_sources: Option<&[String]>,
) {
    let out_of_view = world
        .resources
        .get::<index::RetryIndex>()
        .is_some_and(|idx| idx.is_out_of_view(kind, coords));
    if world.tiles.query::<&TileRequestRetries>(coords).is_none() {
        if let Some(mut tile) = world.tiles.spawn_mut(coords) {
            tile.insert(TileRequestRetries::default());
        }
    }
    if let Some(retries) = world.tiles.query_mut::<&mut TileRequestRetries>(coords) {
        retries
            .slot_mut(kind)
            .mark_started(attempt, out_of_view, only_sources);
        world
            .resources
            .get_or_init_mut::<index::RetryIndex>()
            .mark_pending(kind, coords);
    }
    crate::io::tile_load::issued(world, coords, kind);
}

pub(crate) fn completed(world: &mut World, outcome: TileRequestOutcome) {
    let now = now(world);
    let Some(retries) = world
        .tiles
        .query_mut::<&mut TileRequestRetries>(outcome.coords)
    else {
        return;
    };
    let Some(stale) = retries
        .slot_mut(outcome.kind)
        .begin_completion(outcome.attempt)
    else {
        return;
    };
    let loaded = matches!(outcome.disposition, RequestDisposition::Complete);
    crate::io::tile_load::landed(world, outcome.coords, outcome.kind, !loaded);
    let Some(retries) = world
        .tiles
        .query_mut::<&mut TileRequestRetries>(outcome.coords)
    else {
        world
            .resources
            .get_or_init_mut::<index::RetryIndex>()
            .clear_kind(outcome.kind, outcome.coords);
        return;
    };
    let state = retries.slot_mut(outcome.kind);
    let retried_for = state.apply_completion(outcome.disposition, stale, now);
    let has_deadline = state.has_deadline();
    world
        .resources
        .get_or_init_mut::<index::RetryIndex>()
        .finish_pending(outcome.kind, outcome.coords, has_deadline);
    // The retry is for what the attempt was for, so a data change noted since cannot
    // narrow it to its own sources and lose the ones that failed.
    if let Some(issued_for) = retried_for {
        mark_refresh_sources(world, outcome.coords, issued_for.as_deref());
    }
    crate::io::frame_signals::resource_ready(
        world,
        crate::io::frame_signals::ResourceReady::Tile {
            coords: outcome.coords,
            kind: outcome.kind,
            loaded,
        },
    );
}

/// Payloads from a finished attempt remain valid until a new attempt replaces its identity.
pub(crate) fn accepts(
    world: &World,
    coords: WorldTileCoords,
    kind: RequestKind,
    attempt: Option<u64>,
) -> bool {
    world
        .tiles
        .query::<&TileRequestRetries>(coords)
        .map_or(attempt.is_none(), |retries| {
            retries.slot(kind).matches_attempt(attempt)
        })
}

mod clock;
mod index;
mod rejection;
mod state;

use clock::{now, observed_now};
pub(crate) use index::{cancel_unwanted, queue_cancelled, stop_cancelled, want};
pub(crate) use rejection::{backing_off, rejected};

#[cfg(test)]
mod tests;

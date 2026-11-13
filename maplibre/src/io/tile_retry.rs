//! Completion and bounded retry scheduling for vector, raster and elevation requests.

use std::{cell::Cell, collections::BTreeSet, rc::Rc, time::Duration};

use serde::{Deserialize, Serialize};

use crate::{
    coords::WorldTileCoords,
    io::apc::{IntoMessage, Message, MessageTag},
    tcs::{tiles::TileComponent, world::World},
};

/// Request families whose completion controls a retry deadline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RequestKind {
    /// Decoded map imagery.
    Raster,
    /// Elevation data used for terrain and shading.
    Dem,
    /// Vector geometry and its associated symbol processing.
    Vector,
}

impl MessageTag for RequestKind {
    fn dyn_clone(&self) -> Box<dyn MessageTag> {
        Box::new(*self)
    }
}

impl RequestKind {
    /// Identifies final request results independently from pixel payloads.
    pub fn message_tag(self) -> &'static dyn MessageTag {
        match self {
            Self::Raster => &Self::Raster,
            Self::Dem => &Self::Dem,
            Self::Vector => &Self::Vector,
        }
    }

    pub(crate) fn index(self) -> usize {
        match self {
            Self::Raster => 0,
            Self::Dem => 1,
            Self::Vector => 2,
        }
    }
}

/// Disposition after all sources in one tile request have completed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RequestDisposition {
    /// Every source completed without a transient failure.
    Complete,
    /// At least one source failed temporarily and can be requested again.
    Retry,
}

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

#[derive(Default)]
struct RetryState {
    attempt: Option<u64>,
    pending: bool,
    delay: Duration,
    deadline: Option<Duration>,
    /// The style changed while this request was in flight, so its result is refetched.
    stale: bool,
    /// The sources the attempt was limited to, so that a retry of it is for them again;
    /// `None` for an attempt that processed every source.
    issued_for: Option<Vec<String>>,
    /// The family left the tile out of what it wanted in its last frame, so a passed retry
    /// deadline waits for the tile to come back into view instead of asking for frames.
    out_of_view: bool,
}

impl RetryState {
    /// Whether the next frame has a retry of this tile to issue.
    fn retry_due(&self, now: Duration) -> bool {
        !self.out_of_view && self.deadline.is_some_and(|due| due <= now)
    }

    /// Schedules the next attempt after a delay that doubles with each failure.
    fn back_off(&mut self, now: Duration) {
        self.delay = if self.delay.is_zero() {
            Duration::from_secs(1)
        } else {
            self.delay.saturating_mul(2).min(Duration::from_secs(30))
        };
        self.deadline = Some(now.saturating_add(self.delay));
    }
}

#[derive(Default)]
pub(crate) struct TileRequestRetries([RetryState; 3]);
impl TileComponent for TileRequestRetries {}

// Workers retain the map transport while reset replaces the world and its deadlines.
#[derive(Clone, Default)]
pub(crate) struct RequestAttempts(Rc<Cell<u64>>);

/// Whether a frame has request work: a request in flight, whose result the next frame applies,
/// or a retry whose deadline has passed, which the next frame issues.
pub(crate) fn needs_frame(world: &World) -> bool {
    index::needs_frame_indexed(world)
}

/// Shortest wait until a scheduled retry deadline is due, or `None` when no retry waits.
pub(crate) fn next_retry_delay(world: &World) -> Option<Duration> {
    index::next_retry_delay_indexed(world)
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
    crate::render::frame_signals::mark_dirty(world);
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
        let state = &mut retries.0[kind.index()];
        if state.attempt.is_none() {
            continue;
        }
        if state.pending {
            state.stale = true;
        } else if state.deadline.is_none() {
            // A tile already backing off retries at its deadline with the new data.
            state.delay = Duration::ZERO;
            state.deadline = Some(now);
            state.out_of_view = out_of_view;
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
/// was unavailable for now.
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
    let state = &mut retries.0[kind.index()];
    if state.pending || state.deadline.is_some() {
        return;
    }
    state.deadline = Some(now.saturating_add(delay));
    state.out_of_view = out_of_view;
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
        .is_some_and(|retries| {
            let state = &retries.0[kind.index()];
            !state.pending && state.deadline.is_some_and(|deadline| now >= deadline)
        })
}

#[cfg(any(test, feature = "raster"))]
pub(crate) fn waiting(world: &World, coords: WorldTileCoords, kind: RequestKind) -> bool {
    world
        .tiles
        .query::<&TileRequestRetries>(coords)
        .is_some_and(|retries| {
            let state = &retries.0[kind.index()];
            state.pending || state.deadline.is_some()
        })
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
        let state = &mut retries.0[kind.index()];
        state.attempt = Some(attempt);
        state.pending = true;
        state.deadline = None;
        state.stale = false;
        state.out_of_view = out_of_view;
        state.issued_for = only_sources.map(<[String]>::to_vec);
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
    let state = &mut retries.0[outcome.kind.index()];
    // An evicted request can finish after the same coordinates have been requested again.
    if !state.pending || state.attempt != outcome.attempt {
        return;
    }
    state.pending = false;
    let stale = std::mem::take(&mut state.stale);
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
    let state = &mut retries.0[outcome.kind.index()];
    let mut retried_for = None;
    match outcome.disposition {
        RequestDisposition::Complete => {
            state.delay = Duration::ZERO;
            state.deadline = stale.then_some(now);
        }
        RequestDisposition::Retry => {
            state.back_off(now);
            retried_for = Some(state.issued_for.clone());
        }
    }
    let has_deadline = state.deadline.is_some();
    world
        .resources
        .get_or_init_mut::<index::RetryIndex>()
        .finish_pending(outcome.kind, outcome.coords, has_deadline);
    // The retry is for what the attempt was for, so a data change noted since cannot
    // narrow it to its own sources and lose the ones that failed.
    if let Some(issued_for) = retried_for {
        mark_refresh_sources(world, outcome.coords, issued_for.as_deref());
    }
    crate::render::frame_signals::resource_ready(
        world,
        crate::render::frame_signals::ResourceReady::Tile {
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
        .and_then(|retries| retries.0[kind.index()].attempt)
        == attempt
}

mod clock;
mod index;
mod rejection;

use clock::{now, observed_now};
pub(crate) use index::{cancel_unwanted, stop_cancelled, want};
pub(crate) use rejection::{backing_off, rejected};

#[cfg(test)]
mod tests;

//! Completion and bounded retry scheduling for vector, raster and elevation requests.

use std::{cell::Cell, rc::Rc, time::Duration};

use instant::Instant;
use serde::{Deserialize, Serialize};

use crate::{
    coords::WorldTileCoords,
    io::apc::{IntoMessage, Message, MessageTag},
    render::frame_input::FrameInput,
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

    fn index(self) -> usize {
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
}

#[derive(Default)]
pub(crate) struct TileRequestRetries([RetryState; 3]);
impl TileComponent for TileRequestRetries {}

impl TileRequestRetries {
    pub(crate) fn pending(&self) -> bool {
        self.0.iter().any(|state| state.pending)
    }
}

// Workers retain the map transport while reset replaces the world and its deadlines.
#[derive(Clone, Default)]
pub(crate) struct RequestAttempts(Rc<Cell<u64>>);

struct RequestClock {
    started: Instant,
    last: Duration,
}

impl Default for RequestClock {
    fn default() -> Self {
        Self {
            started: Instant::now(),
            last: Duration::ZERO,
        }
    }
}

fn now(world: &mut World) -> Duration {
    let timestamp = world
        .resources
        .get::<FrameInput>()
        .map(|input| input.timestamp);
    let clock = world.resources.get_or_init_mut::<RequestClock>();
    clock.last = clock
        .last
        .max(timestamp.unwrap_or_else(|| clock.started.elapsed()));
    clock.last
}

/// Whether a frame has request work: a request in flight, whose result the next frame applies,
/// or a retry whose deadline has passed, which the next frame issues.
pub(crate) fn needs_frame(world: &World) -> bool {
    let now = world
        .resources
        .get::<RequestClock>()
        .map_or(Duration::ZERO, |clock| {
            clock.last.max(clock.started.elapsed())
        });
    world.tiles.tiles.values().any(|tile| {
        world
            .tiles
            .query::<&TileRequestRetries>(tile.coords)
            .is_some_and(|retries| {
                retries
                    .0
                    .iter()
                    .any(|state| state.pending || state.deadline.is_some_and(|due| due <= now))
            })
    })
}

/// Requests every requested tile of `kind` again, keeping what it shows until the new data lands.
pub(crate) fn refresh(world: &mut World, kind: RequestKind) {
    crate::render::frame_signals::mark_dirty(world);
    let now = now(world);
    let coords: Vec<WorldTileCoords> = world.tiles.tiles.values().map(|tile| tile.coords).collect();
    for coords in coords {
        let Some(retries) = world.tiles.query_mut::<&mut TileRequestRetries>(coords) else {
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
        }
    }
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

pub(crate) fn started(world: &mut World, coords: WorldTileCoords, kind: RequestKind, attempt: u64) {
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
    }
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
    match outcome.disposition {
        RequestDisposition::Complete => {
            state.delay = Duration::ZERO;
            state.deadline = stale.then_some(now);
        }
        RequestDisposition::Retry => {
            state.delay = if state.delay.is_zero() {
                Duration::from_secs(1)
            } else {
                state.delay.saturating_mul(2).min(Duration::from_secs(30))
            };
            state.deadline = Some(now.saturating_add(state.delay));
        }
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

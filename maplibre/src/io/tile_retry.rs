//! Completion and bounded retry scheduling for raster and elevation requests.

use std::time::Duration;

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
        }
    }

    fn index(self) -> usize {
        match self {
            Self::Raster => 0,
            Self::Dem => 1,
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
    /// Imagery or elevation request family.
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
}

#[derive(Default)]
pub(crate) struct TileRequestRetries([RetryState; 2]);
impl TileComponent for TileRequestRetries {}

impl TileRequestRetries {
    pub(crate) fn pending(&self) -> bool {
        self.0.iter().any(|state| state.pending)
    }
}

struct RequestClock {
    started: Instant,
    last: Duration,
    next_attempt: u64,
}

impl Default for RequestClock {
    fn default() -> Self {
        Self {
            started: Instant::now(),
            last: Duration::ZERO,
            next_attempt: 0,
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

pub(crate) fn next_attempt(world: &mut World) -> u64 {
    let clock = world.resources.get_or_init_mut::<RequestClock>();
    clock.next_attempt = clock.next_attempt.wrapping_add(1);
    clock.next_attempt
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
    match outcome.disposition {
        RequestDisposition::Complete => {
            state.delay = Duration::ZERO;
            state.deadline = None;
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
}

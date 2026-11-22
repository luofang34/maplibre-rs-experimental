//! Sparse active-request index for `O(|in_flight| + |retrying|)` per-frame scheduling.

use std::{collections::HashSet, time::Duration};

use rustc_hash::FxHashSet;

use super::{observed_now, RequestKind, RetryState, TileRequestRetries};
use crate::{coords::WorldTileCoords, tcs::world::World};

/// Tracks only tiles with an in-flight attempt or an active retry deadline per [`RequestKind`],
/// avoiding full-store scans across settled cached tiles on every frame.
#[derive(Default)]
pub(super) struct RetryIndex {
    /// Tiles whose `RetryState::pending` is `true` for each [`RequestKind::index`].
    pub(super) pending: [FxHashSet<WorldTileCoords>; 3],
    /// Tiles whose `RetryState::deadline` is `Some(_)` for each [`RequestKind::index`].
    pub(super) scheduled: [FxHashSet<WorldTileCoords>; 3],
    /// Last wanted set recorded by [`want`] for each [`RequestKind::index`].
    pub(super) last_wanted: [Option<FxHashSet<WorldTileCoords>>; 3],
    active_scratch: Vec<WorldTileCoords>,
    missing_scratch: Vec<WorldTileCoords>,
}

/// The tiles each request family considered this frame; a family that did not run leaves
/// its requests alone.
#[derive(Default)]
struct WantedRequests([Option<FxHashSet<WorldTileCoords>>; 3]);

/// Attempts given up on, for the next request system to stop in its worker.
#[derive(Default)]
struct CancelledAttempts(Vec<u64>);

fn slot<T>(arr: &[T; 3], kind: RequestKind) -> &T {
    let [raster, dem, vector] = arr;
    match kind {
        RequestKind::Raster => raster,
        RequestKind::Dem => dem,
        RequestKind::Vector => vector,
    }
}

fn slot_mut<T>(arr: &mut [T; 3], kind: RequestKind) -> &mut T {
    let [raster, dem, vector] = arr;
    match kind {
        RequestKind::Raster => raster,
        RequestKind::Dem => dem,
        RequestKind::Vector => vector,
    }
}

const REQUEST_KINDS: [RequestKind; 3] =
    [RequestKind::Raster, RequestKind::Dem, RequestKind::Vector];

impl RetryIndex {
    /// Whether `coords` is currently outside the most recent `want()` set for `kind`.
    pub(super) fn is_out_of_view(&self, kind: RequestKind, coords: WorldTileCoords) -> bool {
        slot(&self.last_wanted, kind)
            .as_ref()
            .is_some_and(|wanted| !wanted.contains(&coords))
    }

    pub(super) fn mark_pending(&mut self, kind: RequestKind, coords: WorldTileCoords) {
        slot_mut(&mut self.scheduled, kind).remove(&coords);
        slot_mut(&mut self.pending, kind).insert(coords);
    }

    pub(super) fn finish_pending(
        &mut self,
        kind: RequestKind,
        coords: WorldTileCoords,
        has_deadline: bool,
    ) {
        slot_mut(&mut self.pending, kind).remove(&coords);
        let scheduled = slot_mut(&mut self.scheduled, kind);
        if has_deadline {
            scheduled.insert(coords);
        } else {
            scheduled.remove(&coords);
        }
    }

    pub(super) fn mark_scheduled(&mut self, kind: RequestKind, coords: WorldTileCoords) {
        slot_mut(&mut self.scheduled, kind).insert(coords);
    }

    pub(super) fn clear_kind(&mut self, kind: RequestKind, coords: WorldTileCoords) {
        slot_mut(&mut self.pending, kind).remove(&coords);
        slot_mut(&mut self.scheduled, kind).remove(&coords);
    }
}

pub(super) fn needs_frame_indexed(world: &World) -> bool {
    let Some(index) = world.resources.get::<RetryIndex>() else {
        return false;
    };
    for (kind, pending) in REQUEST_KINDS.into_iter().zip(&index.pending) {
        for &coords in pending {
            if world
                .tiles
                .query::<&TileRequestRetries>(coords)
                .is_some_and(|retries| retries.slot(kind).pending)
            {
                return true;
            }
        }
    }
    if index.scheduled.iter().all(FxHashSet::is_empty) {
        return false;
    }
    let now = observed_now(world);
    for (kind, scheduled) in REQUEST_KINDS.into_iter().zip(&index.scheduled) {
        for &coords in scheduled {
            if world
                .tiles
                .query::<&TileRequestRetries>(coords)
                .is_some_and(|retries| retries.slot(kind).retry_due(now))
            {
                return true;
            }
        }
    }
    false
}

pub(super) fn next_retry_delay_indexed(world: &World) -> Option<Duration> {
    let index = world.resources.get::<RetryIndex>()?;
    if index.scheduled.iter().all(FxHashSet::is_empty) {
        return None;
    }
    let now = observed_now(world);
    let mut shortest: Option<Duration> = None;
    for (kind, scheduled) in REQUEST_KINDS.into_iter().zip(&index.scheduled) {
        for &coords in scheduled {
            let Some(retries) = world.tiles.query::<&TileRequestRetries>(coords) else {
                continue;
            };
            let state = retries.slot(kind);
            if !state.out_of_view {
                if let Some(due) = state.deadline {
                    let wait = due.saturating_sub(now);
                    shortest = Some(shortest.map_or(wait, |prev| prev.min(wait)));
                }
            }
        }
    }
    shortest
}

/// Records the tiles a request family still wants, whether or not it requested them now.
pub(crate) fn want<S: std::hash::BuildHasher>(
    world: &mut World,
    kind: RequestKind,
    coords: &HashSet<WorldTileCoords, S>,
) {
    let has_active = world
        .resources
        .get::<RetryIndex>()
        .is_some_and(|retry_idx| {
            !slot(&retry_idx.pending, kind).is_empty()
                || !slot(&retry_idx.scheduled, kind).is_empty()
        });
    if has_active {
        let crate::tcs::world::World { resources, tiles } = world;
        let retry_idx = resources.get_or_init_mut::<RetryIndex>();
        let mut active = std::mem::take(&mut retry_idx.active_scratch);
        active.clear();
        active.extend(
            slot(&retry_idx.pending, kind)
                .iter()
                .chain(slot(&retry_idx.scheduled, kind).iter())
                .copied(),
        );
        let mut missing = std::mem::take(&mut retry_idx.missing_scratch);
        missing.clear();
        for &held in &active {
            if let Some(retries) = tiles.query_mut::<&mut TileRequestRetries>(held) {
                retries.slot_mut(kind).out_of_view = !coords.contains(&held);
            } else {
                missing.push(held);
            }
        }
        for &gone in &missing {
            retry_idx.clear_kind(kind, gone);
        }
        active.clear();
        retry_idx.active_scratch = active;
        missing.clear();
        retry_idx.missing_scratch = missing;
    }
    let retry_unchanged = world
        .resources
        .get::<RetryIndex>()
        .and_then(|r| slot(&r.last_wanted, kind).as_ref())
        .is_some_and(|prev| prev.len() == coords.len() && coords.iter().all(|c| prev.contains(c)));
    if !retry_unchanged {
        let retry_idx = world.resources.get_or_init_mut::<RetryIndex>();
        let want_slot = slot_mut(&mut retry_idx.last_wanted, kind).get_or_insert_with(|| {
            FxHashSet::with_capacity_and_hasher(coords.len(), Default::default())
        });
        want_slot.clear();
        want_slot.extend(coords.iter().copied());
    }
    let wanted_unchanged = world
        .resources
        .get::<WantedRequests>()
        .and_then(|w| slot(&w.0, kind).as_ref())
        .is_some_and(|prev| prev.len() == coords.len() && coords.iter().all(|c| prev.contains(c)));
    if !wanted_unchanged {
        let wanted = world.resources.get_or_init_mut::<WantedRequests>();
        let want_slot = slot_mut(&mut wanted.0, kind).get_or_insert_with(|| {
            FxHashSet::with_capacity_and_hasher(coords.len(), Default::default())
        });
        want_slot.clear();
        want_slot.extend(coords.iter().copied());
    }
}

/// Gives up the requests in flight that no request family wanted this frame, so they stop
/// holding a slot, and returns the tiles they were for. Tiles in `in_use` keep theirs: the
/// drape requests a family follows lag a frame behind what the view uses.
pub(crate) fn cancel_unwanted<S: std::hash::BuildHasher>(
    world: &mut World,
    in_use: &HashSet<WorldTileCoords, S>,
) -> Vec<WorldTileCoords> {
    if world
        .resources
        .get::<RetryIndex>()
        .is_none_or(|idx| idx.pending.iter().all(FxHashSet::is_empty))
    {
        return Vec::new();
    }
    let Some(wanted) = world
        .resources
        .get_mut::<WantedRequests>()
        .map(std::mem::take)
    else {
        return Vec::new();
    };
    let mut candidate_set = FxHashSet::default();
    if let Some(retry_idx) = world.resources.get::<RetryIndex>() {
        for pending in &retry_idx.pending {
            for &coords in pending {
                if !in_use.contains(&coords) {
                    candidate_set.insert(coords);
                }
            }
        }
    }
    if candidate_set.is_empty() {
        return Vec::new();
    }
    let mut coords_list: Vec<WorldTileCoords> = candidate_set.into_iter().collect();
    coords_list.sort_by_key(|c| c.build_quad_key());
    let mut cancelled = Vec::new();
    let mut tiles = Vec::new();
    let mut given_up = Vec::new();
    let mut missing = Vec::new();
    for coords in coords_list {
        let Some(retries) = world.tiles.query_mut::<&mut TileRequestRetries>(coords) else {
            missing.push(coords);
            continue;
        };
        for ((state, wanted), kind) in retries.0.iter_mut().zip(&wanted.0).zip([
            RequestKind::Raster,
            RequestKind::Dem,
            RequestKind::Vector,
        ]) {
            let unwanted = wanted.as_ref().is_some_and(|set| !set.contains(&coords));
            if state.pending && unwanted {
                cancelled.extend(state.attempt);
                *state = RetryState::default();
                given_up.push((coords, kind));
                if !tiles.contains(&coords) {
                    tiles.push(coords);
                }
            }
        }
    }
    if !given_up.is_empty() || !missing.is_empty() {
        let retry_idx = world.resources.get_or_init_mut::<RetryIndex>();
        for &(coords, kind) in &given_up {
            retry_idx.clear_kind(kind, coords);
        }
        for gone in missing {
            for kind in [RequestKind::Raster, RequestKind::Dem, RequestKind::Vector] {
                retry_idx.clear_kind(kind, gone);
            }
        }
    }
    for (coords, kind) in given_up {
        crate::io::tile_load::abandoned(world, coords, kind);
    }
    queue_cancelled(world, cancelled);
    tiles
}

/// Records attempts to stop in the worker on the next cancellation drain.
pub(crate) fn queue_cancelled(world: &mut World, attempts: impl IntoIterator<Item = u64>) {
    let mut iter = attempts.into_iter().peekable();
    if iter.peek().is_none() {
        return;
    }
    world
        .resources
        .get_or_init_mut::<CancelledAttempts>()
        .0
        .extend(iter);
}

/// Stops the worker calls of the attempts cancelled since the last call.
pub(crate) fn stop_cancelled<K: crate::environment::OffscreenKernel>(
    world: &mut World,
    apc: &impl crate::io::apc::AsyncProcedureCall<K>,
) {
    let mut cancelled = world
        .resources
        .get_mut::<CancelledAttempts>()
        .map(|cancelled| std::mem::take(&mut cancelled.0))
        .unwrap_or_default();
    if cancelled.is_empty() {
        return;
    }
    cancelled.sort_unstable_by(|a, b| b.cmp(a));
    cancelled.dedup();
    apc.cancel_batch(&cancelled);
}

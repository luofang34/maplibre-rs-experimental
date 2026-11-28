//! Sparse active-request index for `O(|in_flight| + |retrying|)` per-frame scheduling.

use std::{collections::HashSet, time::Duration};

use rustc_hash::FxHashSet;

use super::{observed_now, RequestKind, TileRequestRetries};
use crate::{coords::WorldTileCoords, tcs::world::World};

/// Tracks only tiles with an in-flight attempt or an active retry deadline per [`RequestKind`],
/// avoiding full-store scans across settled cached tiles on every frame.
#[derive(Default)]
pub(super) struct RetryIndex {
    /// Tiles whose retry state is pending for each [`RequestKind::index`].
    pending: [FxHashSet<WorldTileCoords>; 3],
    /// Tiles whose retry state has a deadline for each [`RequestKind::index`].
    scheduled: [FxHashSet<WorldTileCoords>; 3],
    /// Last wanted set recorded by [`want`] for each [`RequestKind::index`].
    last_wanted: [Option<FxHashSet<WorldTileCoords>>; 3],
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
    fn has_active(&self, kind: RequestKind) -> bool {
        !slot(&self.pending, kind).is_empty() || !slot(&self.scheduled, kind).is_empty()
    }

    fn has_any_pending(&self) -> bool {
        self.pending.iter().any(|set| !set.is_empty())
    }

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

fn has_pending_tile(world: &World, index: &RetryIndex) -> bool {
    REQUEST_KINDS
        .into_iter()
        .zip(&index.pending)
        .any(|(kind, pending)| {
            pending.iter().any(|&coords| {
                world
                    .tiles
                    .query::<&TileRequestRetries>(coords)
                    .is_some_and(|retries| retries.slot(kind).is_pending())
            })
        })
}

fn has_due_scheduled_tile(world: &World, index: &RetryIndex, now: Duration) -> bool {
    REQUEST_KINDS
        .into_iter()
        .zip(&index.scheduled)
        .any(|(kind, scheduled)| {
            scheduled.iter().any(|&coords| {
                world
                    .tiles
                    .query::<&TileRequestRetries>(coords)
                    .is_some_and(|retries| retries.slot(kind).retry_due(now))
            })
        })
}

pub(super) fn needs_frame_indexed(world: &World) -> bool {
    let Some(index) = world.resources.get::<RetryIndex>() else {
        return false;
    };
    if has_pending_tile(world, index) {
        return true;
    }
    if index.scheduled.iter().all(FxHashSet::is_empty) {
        return false;
    }
    has_due_scheduled_tile(world, index, observed_now(world))
}

pub(super) fn kind_has_due_retry_indexed(world: &World, kind: RequestKind) -> bool {
    let Some(index) = world.resources.get::<RetryIndex>() else {
        return false;
    };
    let scheduled = slot(&index.scheduled, kind);
    if scheduled.is_empty() {
        return false;
    }
    let now = observed_now(world);
    scheduled.iter().any(|&coords| {
        world
            .tiles
            .query::<&TileRequestRetries>(coords)
            .is_some_and(|retries| retries.slot(kind).retry_due(now))
    })
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
            if let Some(due) = state.visible_deadline() {
                let wait = due.saturating_sub(now);
                shortest = Some(shortest.map_or(wait, |prev| prev.min(wait)));
            }
        }
    }
    shortest
}

fn sync_out_of_view_flags<S: std::hash::BuildHasher>(
    world: &mut World,
    kind: RequestKind,
    coords: &HashSet<WorldTileCoords, S>,
) {
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
            if !retries
                .slot_mut(kind)
                .sync_view_presence(coords.contains(&held))
            {
                missing.push(held);
            }
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

fn set_matches<S: std::hash::BuildHasher>(
    prev: Option<&FxHashSet<WorldTileCoords>>,
    coords: &HashSet<WorldTileCoords, S>,
) -> bool {
    prev.is_some_and(|set| set.len() == coords.len() && coords.iter().all(|c| set.contains(c)))
}

fn replace_wanted_set<S: std::hash::BuildHasher>(
    slot: &mut Option<FxHashSet<WorldTileCoords>>,
    coords: &HashSet<WorldTileCoords, S>,
) {
    let want_slot = slot.get_or_insert_with(|| {
        FxHashSet::with_capacity_and_hasher(coords.len(), Default::default())
    });
    want_slot.clear();
    want_slot.extend(coords.iter().copied());
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
        .is_some_and(|retry_idx| retry_idx.has_active(kind));
    if has_active {
        sync_out_of_view_flags(world, kind, coords);
    }
    let retry_unchanged = world
        .resources
        .get::<RetryIndex>()
        .is_some_and(|r| set_matches(slot(&r.last_wanted, kind).as_ref(), coords));
    if !retry_unchanged {
        let retry_idx = world.resources.get_or_init_mut::<RetryIndex>();
        replace_wanted_set(slot_mut(&mut retry_idx.last_wanted, kind), coords);
    }
    let wanted_unchanged = world
        .resources
        .get::<WantedRequests>()
        .is_some_and(|w| set_matches(slot(&w.0, kind).as_ref(), coords));
    if !wanted_unchanged {
        let wanted = world.resources.get_or_init_mut::<WantedRequests>();
        replace_wanted_set(slot_mut(&mut wanted.0, kind), coords);
    }
}

struct CancelUnwantedScratch {
    cancelled: Vec<u64>,
    tiles: Vec<WorldTileCoords>,
    given_up: Vec<(WorldTileCoords, RequestKind)>,
    missing: Vec<WorldTileCoords>,
}

fn collect_unwanted_candidates<S: std::hash::BuildHasher>(
    index: &RetryIndex,
    in_use: &HashSet<WorldTileCoords, S>,
) -> Vec<WorldTileCoords> {
    let mut candidate_set = FxHashSet::default();
    for pending in &index.pending {
        for &coords in pending {
            if !in_use.contains(&coords) {
                candidate_set.insert(coords);
            }
        }
    }
    let mut coords_list: Vec<WorldTileCoords> = candidate_set.into_iter().collect();
    coords_list.sort_by_key(|c| c.build_quad_key());
    coords_list
}

fn cancel_tile_unwanted_kinds(
    world: &mut World,
    coords: WorldTileCoords,
    wanted: &WantedRequests,
    out: &mut CancelUnwantedScratch,
) {
    let Some(retries) = world.tiles.query_mut::<&mut TileRequestRetries>(coords) else {
        out.missing.push(coords);
        return;
    };
    for ((state, wanted_set), kind) in retries
        .slots_mut()
        .iter_mut()
        .zip(&wanted.0)
        .zip(REQUEST_KINDS)
    {
        let unwanted = wanted_set
            .as_ref()
            .is_some_and(|set| !set.contains(&coords));
        if unwanted {
            if let Some(attempt) = state.cancel_if_pending() {
                out.cancelled.extend(attempt);
                out.given_up.push((coords, kind));
                if !out.tiles.contains(&coords) {
                    out.tiles.push(coords);
                }
            }
        }
    }
}

fn apply_cancelled_index_updates(
    world: &mut World,
    given_up: &[(WorldTileCoords, RequestKind)],
    missing: &[WorldTileCoords],
) {
    if given_up.is_empty() && missing.is_empty() {
        return;
    }
    let retry_idx = world.resources.get_or_init_mut::<RetryIndex>();
    for &(coords, kind) in given_up {
        retry_idx.clear_kind(kind, coords);
    }
    for &gone in missing {
        for kind in REQUEST_KINDS {
            retry_idx.clear_kind(kind, gone);
        }
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
        .is_none_or(|idx| !idx.has_any_pending())
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
    let Some(coords_list) = world
        .resources
        .get::<RetryIndex>()
        .map(|idx| collect_unwanted_candidates(idx, in_use))
        .filter(|list| !list.is_empty())
    else {
        return Vec::new();
    };
    let mut out = CancelUnwantedScratch {
        cancelled: Vec::new(),
        tiles: Vec::new(),
        given_up: Vec::new(),
        missing: Vec::new(),
    };
    for coords in coords_list {
        cancel_tile_unwanted_kinds(world, coords, &wanted, &mut out);
    }
    apply_cancelled_index_updates(world, &out.given_up, &out.missing);
    for (coords, kind) in out.given_up {
        crate::io::tile_load::abandoned(world, coords, kind);
    }
    queue_cancelled(world, out.cancelled);
    out.tiles
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
pub(crate) fn stop_cancelled<K: crate::io::apc::OffscreenKernel>(
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

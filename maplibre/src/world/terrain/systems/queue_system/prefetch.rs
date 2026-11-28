//! Keeps a small prepared tile margin without competing with visible terrain.
use std::{collections::HashSet, time::Duration};

use crate::{
    coords::WorldTileCoords,
    render::{
        frame_input::FrameInput,
        memory_budget::{MemoryBudget, MemoryPressure},
        view_state::{ViewState, ViewStatePadding},
        xr::PrefetchView,
    },
    style::Style,
    tcs::{memory_budget::TileBytesEstimator, tiles::Tiles, world::World},
    terrain::request_system::DrapePrefetchRequests,
};

const PREFETCH_BYTES: usize = 32 << 20;
const STANDARD_PREFETCH_BYTES: usize = 64 << 20;

#[derive(Default)]
struct PrefetchCadence(Option<Duration>);

pub(crate) fn prepare(
    style: &Style,
    view: &ViewState,
    world: &mut World,
    visible: &[WorldTileCoords],
    (memory, drape_targets): (MemoryBudget, usize),
) {
    let Some(limit) = prefetch_limit(view, world, memory) else {
        world.resources.insert(DrapePrefetchRequests::default());
        return;
    };
    let min_interval = if !view.has_external_view() && has_ahead_view(world) {
        Duration::from_millis(32)
    } else {
        Duration::from_millis(100)
    };
    let time = world
        .resources
        .get::<FrameInput>()
        .map_or(Duration::ZERO, |input| input.timestamp);
    let cadence = world.resources.get_or_init_mut::<PrefetchCadence>();
    if cadence
        .0
        .is_some_and(|previous| time >= previous && time - previous < min_interval)
    {
        return;
    }
    cadence.0 = Some(time);
    let candidates = collect_prefetch_candidates(style, view, world, drape_targets);
    let max_bytes = if view.has_external_view() {
        PREFETCH_BYTES
    } else {
        STANDARD_PREFETCH_BYTES
    };
    let estimate_tile_bytes: fn(&Tiles, WorldTileCoords) -> usize = world
        .resources
        .get::<TileBytesEstimator>()
        .map_or(|_, _| 0, |estimator| estimator.0);
    let candidates = select_with_budget(candidates, visible, limit, max_bytes, |tile| {
        estimate_tile_bytes(&world.tiles, tile)
    });
    world.resources.insert(DrapePrefetchRequests(candidates));
}

fn has_ahead_view(world: &World) -> bool {
    world
        .resources
        .get::<PrefetchView>()
        .is_some_and(|prefetch| prefetch.view_state.is_some())
}

fn prefetch_limit(view: &ViewState, world: &World, memory: MemoryBudget) -> Option<usize> {
    if world
        .resources
        .get::<crate::render::settings::TightTileRequests>()
        .is_some_and(|tight| tight.0)
    {
        return None;
    }
    let limit = match memory.pressure() {
        MemoryPressure::Comfortable => 8,
        MemoryPressure::Low => 4,
        MemoryPressure::Critical => 0,
    };
    if limit == 0
        || (cfg!(target_arch = "wasm32") && !view.has_external_view() && !has_ahead_view(world))
    {
        return None;
    }
    Some(limit)
}

fn collect_prefetch_candidates(
    style: &Style,
    view: &ViewState,
    world: &World,
    drape_targets: usize,
) -> Vec<WorldTileCoords> {
    let ahead = world
        .resources
        .get::<PrefetchView>()
        .and_then(|prefetch| prefetch.view_state.as_ref());
    let (request_view, padding) = if view.has_external_view() {
        (
            ahead.unwrap_or(view),
            if ahead.is_some() {
                ViewStatePadding::Tight
            } else {
                ViewStatePadding::Loose
            },
        )
    } else {
        (view, ViewStatePadding::Loose)
    };
    let region = crate::terrain::source::dem_source(style)
        .map(|dem| {
            crate::render::projection::terrain_region(
                style,
                request_view,
                world,
                dem.tile_size,
                padding,
            )
        })
        .transpose()
        .map(Option::flatten)
        .map_err(|error| {
            tracing::warn!(%error, "cannot prepare terrain margin");
        })
        .ok()
        .flatten();
    let tiles = region.map_or_else(Vec::new, |region| region.iter().collect());
    let focus = super::covering::focus_for_view(style, request_view);
    super::covering::bounded_covering_with_focus(tiles.into_iter(), drape_targets, focus)
}

#[cfg(test)]
fn select(
    candidates: Vec<WorldTileCoords>,
    visible: &[WorldTileCoords],
    limit: usize,
    bytes: impl Fn(WorldTileCoords) -> usize,
) -> Vec<WorldTileCoords> {
    select_with_budget(candidates, visible, limit, PREFETCH_BYTES, bytes)
}

fn select_with_budget(
    candidates: Vec<WorldTileCoords>,
    visible: &[WorldTileCoords],
    limit: usize,
    max_bytes: usize,
    bytes: impl Fn(WorldTileCoords) -> usize,
) -> Vec<WorldTileCoords> {
    let visible: HashSet<_> = visible.iter().copied().collect();
    let mut used = 0usize;
    candidates
        .into_iter()
        .filter(|tile| !visible.contains(tile))
        .take(limit)
        .take_while(|tile| {
            // Reserve space for a not-yet-decoded tile instead of treating it as free.
            used = used.saturating_add(bytes(*tile).max(8 << 20));
            used <= max_bytes
        })
        .collect()
}

#[cfg(test)]
mod tests;

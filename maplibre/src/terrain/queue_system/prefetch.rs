//! Keeps a small prepared tile margin without competing with visible terrain.
use std::{collections::HashSet, time::Duration};

use crate::{
    coords::WorldTileCoords,
    render::{
        frame_input::FrameInput,
        memory_budget::{MemoryBudget, MemoryPressure},
        tile_memory::tile_bytes,
        view_state::{ViewState, ViewStatePadding},
        xr::PrefetchView,
    },
    style::Style,
    tcs::world::World,
    terrain::request_system::DrapePrefetchRequests,
};

const PREFETCH_BYTES: usize = 32 << 20;
const STANDARD_PREFETCH_BYTES: usize = 64 << 20;

#[derive(Default)]
struct PrefetchCadence(Option<Duration>);

pub(super) fn prepare(
    style: &Style,
    view: &ViewState,
    world: &mut World,
    visible: &[WorldTileCoords],
    memory: MemoryBudget,
) {
    let limit = match memory.pressure() {
        MemoryPressure::Comfortable => 8,
        MemoryPressure::Low => 4,
        MemoryPressure::Critical => 0,
    };
    if limit == 0 {
        world.resources.insert(DrapePrefetchRequests::default());
        return;
    }
    let has_ahead = world
        .resources
        .get::<PrefetchView>()
        .is_some_and(|prefetch| prefetch.view_state.is_some());
    let min_interval = if !view.has_external_view() && has_ahead {
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
    let candidates = {
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
        super::covering::bounded_covering_with_focus(
            tiles.into_iter(),
            memory.drape_textures_allowed().saturating_sub(8).max(1),
            focus,
        )
    };
    let max_bytes = if view.has_external_view() {
        PREFETCH_BYTES
    } else {
        STANDARD_PREFETCH_BYTES
    };
    let candidates = select_with_budget(candidates, visible, limit, max_bytes, |tile| {
        tile_bytes(&world.tiles, tile)
    });
    world.resources.insert(DrapePrefetchRequests(candidates));
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

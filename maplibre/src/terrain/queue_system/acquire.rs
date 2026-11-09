//! Which drapes a frame acquires and draws: textures for its targets, the redraws its time
//! budget holds, and the drape each surface shows meanwhile.

use std::collections::HashSet;

use super::{covering, present_ancestors};
use crate::{
    coords::WorldTileCoords,
    render::memory_budget::MemoryBudget,
    terrain::{drape_cache::DrapeState, drape_targets::TargetSpec, resources::TerrainResources},
};

/// Which of the acquired drapes to draw this frame, at most `budget`: tiles never drawn
/// first, since a reused texture shows another tile's content until they are, then tiles
/// whose content changed.
pub(super) fn budget_redraws(states: &[DrapeState], budget: usize) -> Vec<bool> {
    let mut redraw = vec![false; states.len()];
    let mut drawn = 0;
    for wanted in [DrapeState::New, DrapeState::Changed] {
        for (index, state) in states.iter().enumerate() {
            if drawn >= budget {
                return redraw;
            }
            if *state == wanted {
                redraw[index] = true;
                drawn += 1;
            }
        }
    }
    redraw
}

/// Tiles whose texture holds no content of their own yet. A new drape the budget deferred
/// would show whatever tile last used the texture, so its surface uses an ancestor or the
/// background color until its own texture is drawn.
pub(super) fn awaiting_first_draw(states: &[DrapeState], redraw: &[bool]) -> Vec<bool> {
    states
        .iter()
        .zip(redraw)
        .map(|(state, drawn)| *state == DrapeState::New && !drawn)
        .collect()
}

pub(super) fn acquire_drapes(
    (specs, extra_retained): (
        &[TargetSpec],
        Option<&rustc_hash::FxHashSet<WorldTileCoords>>,
    ),
    (prints, ready, prefetch): (&[u64], &[bool], &[bool]),
    memory: MemoryBudget,
    (budget, limit): (usize, usize),
    terrain: &mut TerrainResources,
    device: &wgpu::Device,
) -> (Vec<bool>, Vec<Option<WorldTileCoords>>, bool) {
    let keep = covering::retained_textures(
        specs
            .iter()
            .map(|spec| spec.coords)
            .chain(extra_retained.into_iter().flatten().copied()),
        |coords| terrain.drape_texture(coords).is_some(),
    );
    terrain.retain_drapes(&keep);
    // Unready tiles keep their last valid texture or a background surface.
    if memory.is_tight() {
        terrain.shed_spare_drapes();
    }
    // Targets come nearest first, so when textures run out it is the far tiles that
    // draw with an ancestor's drape.
    let states: Vec<DrapeState> = specs
        .iter()
        .zip(prints)
        .zip(ready)
        .zip(prefetch)
        .map(|(((spec, print), ready), prefetch)| {
            if !*ready {
                return DrapeState::Unchanged;
            }
            let may_create = memory.allows_drape_texture(terrain.drape_texture_total());
            if *prefetch {
                terrain.acquire_ahead_drape(device, spec.coords, *print, may_create)
            } else {
                terrain.acquire_drape(device, spec.coords, *print, may_create)
            }
        })
        .collect();
    let redraw = choose_redraws(specs, &states, prefetch, (budget, limit), terrain);
    let mut deferred = false;
    for ((spec, state), drawn) in specs.iter().zip(&states).zip(&redraw) {
        if !matches!(state, DrapeState::Unchanged | DrapeState::Withheld) && !drawn {
            terrain.defer_drape(spec.coords, *state);
            deferred = true;
        }
    }
    let sources = drape_sources(specs, &states, &redraw, ready, terrain);
    (redraw, sources, deferred)
}

/// The drapes to draw this frame. Those the view shows take the budget first; a new one with
/// no drawn ancestor would leave its tile blank, so it is drawn past the budget, up to the count
/// a frame's command buffer holds. Prefetched drapes, which no surface shows yet, draw only
/// with the budget the view leaves and never past it.
fn choose_redraws(
    specs: &[TargetSpec],
    states: &[DrapeState],
    prefetch: &[bool],
    (budget, limit): (usize, usize),
    terrain: &TerrainResources,
) -> Vec<bool> {
    let only = |ahead: bool| -> Vec<DrapeState> {
        states
            .iter()
            .zip(prefetch)
            .map(|(state, prefetched)| {
                if *prefetched == ahead {
                    *state
                } else {
                    DrapeState::Unchanged
                }
            })
            .collect()
    };
    let mut redraw = budget_redraws(&only(false), budget);
    let mut drawn = redraw.iter().filter(|drawn| **drawn).count();
    for (index, (spec, state)) in specs.iter().zip(states).enumerate() {
        if drawn >= limit {
            break;
        }
        if !prefetch[index]
            && *state == DrapeState::New
            && !redraw[index]
            && present_ancestors(spec.coords, terrain).is_empty()
        {
            redraw[index] = true;
            drawn += 1;
        }
    }
    let spare = budget.saturating_sub(drawn);
    for (index, draw) in budget_redraws(&only(true), spare).into_iter().enumerate() {
        redraw[index] |= draw;
    }
    redraw
}

pub(super) fn drape_sources(
    specs: &[TargetSpec],
    states: &[DrapeState],
    redraw: &[bool],
    ready: &[bool],
    terrain: &TerrainResources,
) -> Vec<Option<WorldTileCoords>> {
    let hidden: Vec<bool> = awaiting_first_draw(states, redraw)
        .into_iter()
        .zip(ready)
        .zip(states)
        .map(|((hidden, ready), state)| hidden || !ready || *state == DrapeState::Withheld)
        .collect();
    let undrawn: HashSet<WorldTileCoords> = specs
        .iter()
        .zip(&hidden)
        .filter(|(_, hidden)| **hidden)
        .map(|(spec, _)| spec.coords)
        .collect();
    specs
        .iter()
        .zip(&hidden)
        .map(|(spec, hidden)| {
            if *hidden && terrain.drape_texture(spec.coords).is_none() {
                present_ancestors(spec.coords, terrain)
                    .into_iter()
                    .find(|ancestor| !undrawn.contains(ancestor))
            } else {
                Some(spec.coords)
            }
        })
        .collect()
}

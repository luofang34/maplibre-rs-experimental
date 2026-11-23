//! Which drapes a frame acquires and draws: textures for its targets, the redraws its time
//! budget holds, and the drape each surface shows meanwhile.

use rustc_hash::FxHashSet as HashSet;

use super::{
    covering,
    uniforms::{has_present_ancestor, nearest_present_ancestor},
};
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
        for (slot, state) in redraw.iter_mut().zip(states) {
            if drawn >= budget {
                return redraw;
            }
            if *state == wanted {
                *slot = true;
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
    specs: &[TargetSpec],
    (prints, ready, prefetch): (&[u64], &[bool], &[bool]),
    (memory, view_textures): (MemoryBudget, usize),
    (budget, limit): (usize, usize),
    terrain: &mut TerrainResources,
    device: &wgpu::Device,
    (coalesce, coalesce_motion): (&mut super::steady_cache::DrapeCoalesceState, bool),
) -> (
    Vec<bool>,
    Vec<Option<WorldTileCoords>>,
    bool,
    Vec<DrapeState>,
) {
    let target_coords: Vec<WorldTileCoords> = specs.iter().map(|spec| spec.coords).collect();
    terrain.retain_drapes_if_stale(&target_coords, view_textures, |t| {
        covering::retained_textures_with_zoom_out(&target_coords, ready, view_textures, |coords| {
            t.drape_texture(coords).is_some()
        })
    });
    // Unready tiles keep their last valid texture or a background surface.
    if memory.is_tight() {
        terrain.shed_spare_drapes();
    }
    // Targets come nearest first, so when textures run out it is the far tiles that
    // draw with an ancestor's drape.
    let mut held_drapes = terrain.view_drape_count();
    let states: Vec<DrapeState> = specs
        .iter()
        .zip(prints)
        .zip(ready)
        .zip(prefetch)
        .enumerate()
        .map(|(rank, (((spec, print), ready), prefetch))| {
            if !*ready {
                return DrapeState::Unchanged;
            }
            // Past its share of the textures a view goes without, as it does when the map's
            // allowance is spent, rather than take what another view's share covers.
            if !terrain.holds_drape(spec.coords) && held_drapes >= view_textures {
                return DrapeState::Withheld;
            }
            let may_create = memory.allows_drape_texture(terrain.drape_texture_total());
            let state = if *prefetch {
                terrain.acquire_ahead_drape(device, spec.coords, *print, may_create)
            } else {
                terrain.acquire_drape_ranked(device, spec.coords, rank, *print, may_create)
            };
            if state == DrapeState::New {
                held_drapes = if may_create {
                    held_drapes + 1
                } else {
                    terrain.view_drape_count()
                };
            }
            state
        })
        .collect();
    if ready.iter().all(|&r| r) && states.iter().all(|s| *s == DrapeState::Unchanged) {
        coalesce.reset();
        terrain.finish_drape_retain_state(&target_coords, view_textures, true);
        let redraw = vec![false; specs.len()];
        let sources = specs.iter().map(|s| Some(s.coords)).collect();
        return (redraw, sources, false, states);
    }
    finish_acquire_drapes(
        specs,
        ready,
        prefetch,
        (&target_coords, view_textures, budget, limit),
        states,
        terrain,
        (coalesce, coalesce_motion),
    )
}

fn finish_acquire_drapes(
    specs: &[TargetSpec],
    ready: &[bool],
    prefetch: &[bool],
    (target_coords, view_textures, budget, limit): (&[WorldTileCoords], usize, usize, usize),
    states: Vec<DrapeState>,
    terrain: &mut TerrainResources,
    (coalesce, coalesce_motion): (&mut super::steady_cache::DrapeCoalesceState, bool),
) -> (
    Vec<bool>,
    Vec<Option<WorldTileCoords>>,
    bool,
    Vec<DrapeState>,
) {
    let mut redraw = choose_redraws(specs, &states, prefetch, (budget, limit), terrain);
    let defer_changed =
        coalesce.should_defer_changed(coalesce_motion, states.contains(&DrapeState::Changed));
    if defer_changed {
        for (state, drawn) in states.iter().zip(redraw.iter_mut()) {
            if *state == DrapeState::Changed {
                *drawn = false;
            }
        }
    }
    let mut deferred = false;
    for ((spec, state), drawn) in specs.iter().zip(&states).zip(&redraw) {
        if !matches!(state, DrapeState::Unchanged | DrapeState::Withheld) && !drawn {
            terrain.defer_drape(spec.coords, *state);
            deferred = true;
        }
    }
    let clean = !deferred
        && ready.iter().all(|&r| r)
        && states
            .iter()
            .all(|s| matches!(s, DrapeState::Unchanged | DrapeState::Changed));
    terrain.finish_drape_retain_state(target_coords, view_textures, clean);
    let sources = drape_sources(specs, &states, &redraw, ready, terrain);
    (redraw, sources, deferred, states)
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
    let emergency_limit = if cfg!(target_arch = "wasm32") {
        budget.max(4).min(limit)
    } else {
        limit
    };
    for (((spec, state), &is_prefetch), slot) in specs
        .iter()
        .zip(states)
        .zip(prefetch)
        .zip(redraw.iter_mut())
    {
        if drawn >= emergency_limit {
            break;
        }
        if !is_prefetch
            && *state == DrapeState::New
            && !*slot
            && !has_present_ancestor(spec.coords, terrain)
        {
            *slot = true;
            drawn += 1;
        }
    }
    let spare = budget.saturating_sub(drawn);
    for (slot, draw) in redraw.iter_mut().zip(budget_redraws(&only(true), spare)) {
        *slot |= draw;
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
                nearest_present_ancestor(spec.coords, terrain, |ancestor| {
                    !undrawn.contains(&ancestor)
                })
            } else {
                Some(spec.coords)
            }
        })
        .collect()
}

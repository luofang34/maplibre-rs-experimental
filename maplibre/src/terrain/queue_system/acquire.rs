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
#[cfg(test)]
pub(super) fn budget_redraws(states: &[DrapeState], budget: usize) -> Vec<bool> {
    let mut redraw = vec![false; states.len()];
    fill_budget_redraws(states, None, false, budget, &mut redraw);
    redraw
}

fn fill_budget_redraws(
    states: &[DrapeState],
    prefetch: Option<&[bool]>,
    want_prefetch: bool,
    budget: usize,
    redraw: &mut [bool],
) -> usize {
    let mut drawn = 0;
    for wanted in [DrapeState::New, DrapeState::Changed] {
        for (idx, (slot, state)) in redraw.iter_mut().zip(states).enumerate() {
            if drawn >= budget {
                return drawn;
            }
            let matches_ahead = prefetch
                .is_none_or(|flags| flags.get(idx).copied().unwrap_or(false) == want_prefetch);
            if matches_ahead && *state == wanted {
                *slot = true;
                drawn += 1;
            }
        }
    }
    drawn
}

/// Tiles whose texture holds no content of their own yet. A new drape the budget deferred
/// would show whatever tile last used the texture, so its surface uses an ancestor or the
/// background color until its own texture is drawn.
#[cfg(test)]
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
    let mut target_coords = std::mem::take(&mut coalesce.scratch.target_coords);
    target_coords.clear();
    target_coords.extend(specs.iter().map(|spec| spec.coords));
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
        coalesce.scratch.target_coords = target_coords;
        let redraw = vec![false; specs.len()];
        let sources = specs.iter().map(|s| Some(s.coords)).collect();
        return (redraw, sources, false, states);
    }
    let result = finish_acquire_drapes(
        specs,
        ready,
        prefetch,
        (&target_coords, view_textures, budget, limit),
        states,
        terrain,
        (coalesce, coalesce_motion),
    );
    coalesce.scratch.target_coords = target_coords;
    result
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
    let sources = drape_sources_with_scratch(
        specs,
        &states,
        &redraw,
        ready,
        terrain,
        (&mut coalesce.scratch.hidden, &mut coalesce.scratch.undrawn),
    );
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
    let mut redraw = vec![false; states.len()];
    let mut drawn = fill_budget_redraws(states, Some(prefetch), false, budget, &mut redraw);
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
    if spare > 0 {
        fill_budget_redraws(states, Some(prefetch), true, spare, &mut redraw);
    }
    redraw
}

fn drape_sources_with_scratch(
    specs: &[TargetSpec],
    states: &[DrapeState],
    redraw: &[bool],
    ready: &[bool],
    terrain: &TerrainResources,
    (hidden, undrawn): (&mut Vec<bool>, &mut HashSet<WorldTileCoords>),
) -> Vec<Option<WorldTileCoords>> {
    hidden.clear();
    hidden.extend(
        states
            .iter()
            .zip(redraw)
            .zip(ready)
            .map(|((state, drawn), ready)| {
                (*state == DrapeState::New && !drawn) || !ready || *state == DrapeState::Withheld
            }),
    );
    undrawn.clear();
    undrawn.extend(
        specs
            .iter()
            .zip(hidden.iter())
            .filter(|(_, h)| **h)
            .map(|(spec, _)| spec.coords),
    );
    specs
        .iter()
        .zip(hidden.iter())
        .map(|(spec, is_hidden)| {
            if *is_hidden && terrain.drape_texture(spec.coords).is_none() {
                nearest_present_ancestor(spec.coords, terrain, |ancestor| {
                    !undrawn.contains(&ancestor)
                })
            } else {
                Some(spec.coords)
            }
        })
        .collect()
}

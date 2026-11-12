//! One allowance of drape textures, divided between the views that drape.
//!
//! Memory allows a map so many drape textures, and every view holds its own. Were the views to
//! draw from the allowance as they come, the first of a frame would take what it wants and the
//! far tiles of the next would go without a texture and show an ancestor's. The allowance is
//! instead granted at the start of a frame, by need and in the order of the views' priority:
//! the first view has the textures it needs to drape its ground uncoarsened, as it would alone,
//! and each view after it has what is left, never less than a floor that keeps its ground
//! draped. A view coarsens its drapes to its grant, so the views together never ask for more
//! textures than there are, whatever the device allows and however many views it draws, and
//! what a view does not need is not kept from the others.

use rustc_hash::FxHashMap;

use crate::{
    render::{
        eye_covering::{DrapeView, ViewSlotId},
        memory_budget::MemoryBudget,
    },
    tcs::world::World,
};

/// Textures a view with the whole allowance keeps spare while it regroups its drapes: two
/// groups of four siblings whose parents stay visible until they are drawn. A view with part
/// of the allowance keeps that part of them.
const REGROUPING_TEXTURES: usize = 8;

/// Textures a view is granted however many the views before it need, so its ground is draped,
/// coarsely, rather than bare.
const FLOOR_TEXTURES: usize = 4;

/// Needs are granted in steps of this many textures, so a covering that gains or loses a tile
/// from one frame to the next does not regroup the drapes of the views after it.
const NEED_STEP: usize = 4;

/// What each draping view needs and is granted of the drape texture allowance. A map drawn
/// without view slots has all of it.
#[derive(Clone, Debug, Default)]
pub(crate) struct DrapeShares {
    /// Terrain tiles each view last draped, before coarsening.
    needs: FxHashMap<ViewSlotId, usize>,
    grants: FxHashMap<ViewSlotId, usize>,
}

impl DrapeShares {
    /// Grants `allowed` textures to `views`, which come in the order of their priority. A view
    /// that has not draped yet is taken to need them all.
    pub(crate) fn grant(&mut self, views: &[ViewSlotId], allowed: usize) {
        let needs: Vec<usize> = views
            .iter()
            .map(|view| {
                self.needs.get(view).map_or(allowed, |tiles| {
                    (tiles + REGROUPING_TEXTURES)
                        .next_multiple_of(NEED_STEP)
                        .min(allowed)
                })
            })
            .collect();
        let floors: Vec<usize> = needs.iter().map(|need| FLOOR_TEXTURES.min(*need)).collect();
        let mut left = allowed;
        self.grants.clear();
        for (index, view) in views.iter().enumerate() {
            let after: usize = floors.iter().skip(index + 1).sum();
            let granted = needs[index]
                .min(left.saturating_sub(after))
                .max(floors[index].min(left));
            left -= granted;
            self.grants.insert(*view, granted);
        }
        self.needs.retain(|view, _| views.contains(view));
    }

    /// Records the terrain tiles `view` drapes this frame, for the grants of the next.
    fn need(&mut self, view: ViewSlotId, tiles: usize) {
        self.needs.insert(view, tiles);
    }
}

/// What the view being drawn may hold and aim for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DrapeAllowance {
    /// Drape textures the view may hold.
    pub(crate) textures: usize,
    /// Drapes the view coarsens its covering to, leaving the textures it regroups with.
    pub(crate) targets: usize,
}

/// The allowance of the view being drawn, which drapes `tiles` terrain tiles before
/// coarsening: its grant of what `memory` allows the map.
pub(crate) fn allowance(world: &mut World, memory: MemoryBudget, tiles: usize) -> DrapeAllowance {
    let allowed = memory.drape_textures_allowed();
    let view = world.resources.get::<DrapeView>().and_then(|view| view.0);
    let Some(view) = view else {
        return of_grant(allowed, allowed);
    };
    let shares = world.resources.get_or_init_mut::<DrapeShares>();
    shares.need(view, tiles);
    of_grant(
        shares.grants.get(&view).copied().unwrap_or(allowed),
        allowed,
    )
}

/// The textures the view being drawn may hold.
pub(crate) fn textures(world: &World, memory: MemoryBudget) -> usize {
    let allowed = memory.drape_textures_allowed();
    world
        .resources
        .get::<DrapeView>()
        .and_then(|view| view.0)
        .and_then(|view| {
            world
                .resources
                .get::<DrapeShares>()?
                .grants
                .get(&view)
                .copied()
        })
        .map_or(allowed, |granted| granted.min(allowed).max(1))
}

fn of_grant(granted: usize, allowed: usize) -> DrapeAllowance {
    let textures = granted.min(allowed).max(1);
    let regrouping = (REGROUPING_TEXTURES * textures).div_ceil(allowed.max(1));
    DrapeAllowance {
        textures,
        targets: textures.saturating_sub(regrouping).max(1),
    }
}

#[cfg(test)]
mod tests;

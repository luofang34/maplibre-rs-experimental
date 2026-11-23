//! Reusable per-frame scratch buffers, steady-state terrain draw caching, XR eye-frame
//! gating, and bounded drape redraw coalescing for moving cameras.

use rustc_hash::FxHashMap;

use super::{surface_covering::SurfaceCopies, uniforms::PreparedTerrain};
use crate::{
    coords::WorldTileCoords,
    render::{
        eye_covering::{CurrentViewSlot, EyeInFrame, EyePassCount, MultiViewExtractCovering},
        settings::TerrainDrapeMode,
    },
    tcs::world::World,
    terrain::resources::{TerrainDraw, TerrainResources, UNIFORM_STRIDE},
};

const MAX_COALESCE_FRAMES: u8 = 2;

#[derive(Default)]
pub(super) struct TerrainQueueScratch {
    pub(super) prepared: PreparedTerrain,
    keyed_indices: Vec<(f64, usize)>,
    last_draw_meta: Vec<(usize, WorldTileCoords, Option<u64>, Option<u64>)>,
    last_drape_mode: Option<TerrainDrapeMode>,
    pub(super) copies_scratch: FxHashMap<WorldTileCoords, Vec<i32>>,
}

impl TerrainQueueScratch {
    pub(super) fn populate_copies(&mut self, copies: Option<&SurfaceCopies>) -> bool {
        let Some(copies) = copies.filter(|c| !c.0.is_empty()) else {
            if !self.copies_scratch.is_empty() {
                self.copies_scratch.clear();
            }
            return false;
        };
        self.copies_scratch.retain(|k, _| copies.0.contains_key(k));
        for (&coords, wraps) in &copies.0 {
            let dst = self.copies_scratch.entry(coords).or_default();
            dst.clear();
            dst.extend_from_slice(wraps);
        }
        true
    }

    pub(super) fn sync_draws(
        &mut self,
        device: &wgpu::Device,
        terrain: &mut TerrainResources,
        written: usize,
        key_fn: &impl Fn(&WorldTileCoords) -> f64,
    ) {
        self.keyed_indices.clear();
        for (index, (_, coords, drape_source)) in
            self.prepared.sources.iter().take(written).enumerate()
        {
            if drape_source.is_some_and(|src| terrain.drape_id(src).is_none()) {
                continue;
            }
            self.keyed_indices.push((key_fn(coords), index));
        }
        self.keyed_indices
            .sort_unstable_by(|left, right| left.0.total_cmp(&right.0));
        let mode = terrain.drape_mode();
        if self.draws_match(terrain, mode) {
            return;
        }
        self.rebuild_draws(device, terrain, mode);
    }

    fn draws_match(&self, terrain: &TerrainResources, mode: TerrainDrapeMode) -> bool {
        self.last_drape_mode == Some(mode)
            && terrain.draws().len() == self.keyed_indices.len()
            && self.last_draw_meta.len() == self.keyed_indices.len()
            && self.keyed_indices.iter().zip(&self.last_draw_meta).all(
                |(&(_, index), &(p_idx, p_coords, p_dem, p_drape))| {
                    let Some((_, coords, drape_source)) = self.prepared.sources.get(index) else {
                        return false;
                    };
                    let dem_id = self.prepared.dem_ids.get(index).copied().flatten();
                    let drape_id = drape_source.and_then(|src| terrain.drape_id(src));
                    p_idx == index && p_coords == *coords && p_dem == dem_id && p_drape == drape_id
                },
            )
    }

    fn rebuild_draws(
        &mut self,
        device: &wgpu::Device,
        terrain: &mut TerrainResources,
        mode: TerrainDrapeMode,
    ) {
        self.last_draw_meta.clear();
        self.last_draw_meta.reserve(self.keyed_indices.len());
        let mut draws = Vec::with_capacity(self.keyed_indices.len());
        for &(_, index) in &self.keyed_indices {
            let Some(&(dem_coords, coords, drape_source)) = self.prepared.sources.get(index) else {
                continue;
            };
            let dem_id = self.prepared.dem_ids.get(index).copied().flatten();
            let drape_id = drape_source.and_then(|src| terrain.drape_id(src));
            let Some(bind_group) =
                terrain.tile_bind_group_with_dem_id(device, dem_coords, dem_id, drape_source)
            else {
                continue;
            };
            self.last_draw_meta.push((index, coords, dem_id, drape_id));
            draws.push(TerrainDraw {
                coords,
                bind_group,
                uniform_offset: (index as u64 * UNIFORM_STRIDE) as u32,
            });
        }
        self.last_drape_mode = Some(mode);
        terrain.set_draws(draws);
    }
}

pub(super) fn needs_eye_frame(world: &World) -> bool {
    world.resources.get::<EyeInFrame>().is_some()
        || world
            .resources
            .get::<EyePassCount>()
            .is_some_and(|count| count.0 > 1)
        || world
            .resources
            .get::<CurrentViewSlot>()
            .and_then(|slot| slot.0)
            .is_some()
        || world
            .resources
            .get::<MultiViewExtractCovering>()
            .is_some_and(|multi| multi.active)
}

#[derive(Default)]
pub(super) struct DrapeCoalesceState {
    moving_defer_streak: u8,
}

impl DrapeCoalesceState {
    pub(super) fn reset(&mut self) {
        self.moving_defer_streak = 0;
    }

    pub(super) fn should_defer_changed(
        &mut self,
        coalesce_motion: bool,
        has_changed: bool,
    ) -> bool {
        if !coalesce_motion || !has_changed {
            self.moving_defer_streak = 0;
            return false;
        }
        if self.moving_defer_streak < MAX_COALESCE_FRAMES {
            self.moving_defer_streak += 1;
            true
        } else {
            self.moving_defer_streak = 0;
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::DrapeCoalesceState;

    #[test]
    fn drape_coalesce_defers_at_most_two_moving_frames_and_resets_when_settled() {
        let mut state = DrapeCoalesceState::default();
        assert!(state.should_defer_changed(true, true));
        assert!(state.should_defer_changed(true, true));
        assert!(!state.should_defer_changed(true, true));
        assert!(state.should_defer_changed(true, true));

        assert!(!state.should_defer_changed(false, true));
        assert!(state.should_defer_changed(true, true));

        assert!(!state.should_defer_changed(true, false));
        assert!(state.should_defer_changed(true, true));
    }
}

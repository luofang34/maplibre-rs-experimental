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
const MAX_STATIC_CACHE_ENTRIES: usize = if cfg!(target_arch = "wasm32") {
    512
} else {
    1024
};

pub(super) type StaticTileKey = (
    WorldTileCoords,
    Option<WorldTileCoords>,
    Option<u64>,
    Option<WorldTileCoords>,
);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct StaticFrameKey {
    pub(super) style_rev: u64,
    pub(super) paint_zoom_bits: u64,
    pub(super) clear_color_bits: [u32; 4],
    pub(super) poles_north_bits: u32,
    pub(super) poles_south_bits: u32,
    pub(super) drape_mode_bits: (u8, u64, u32, u32, u32, u32),
    pub(super) dem_exaggeration_bits: u32,
    pub(super) dem_unpack_bits: [u64; 4],
    pub(super) radius_bits: u64,
    pub(super) has_globe_camera: bool,
    pub(super) vector_arrival_epoch: u64,
    pub(super) vector_tiles_len: usize,
}

impl StaticFrameKey {
    pub(super) fn drape_mode_bits(mode: TerrainDrapeMode) -> (u8, u64, u32, u32, u32, u32) {
        match mode {
            TerrainDrapeMode::Draped { edge_px } => (0, u64::from(edge_px), 0, 0, 0, 0),
            TerrainDrapeMode::ProceduralRelief(relief) => (
                1,
                relief
                    .reference_altitude_meters
                    .map_or(0, |v| (1_u64 << 32) | u64::from(v.to_bits())),
                u32::from(relief.hypsometric),
                relief.slope_shading.to_bits(),
                relief.grid_spacing_meters.to_bits(),
                relief.grid_opacity.to_bits(),
            ),
        }
    }
}

#[derive(Clone)]
pub(super) struct CachedTileStatic {
    pub(super) dem_matrix: [[f32; 4]; 4],
    pub(super) drape_matrix: [[f32; 4]; 4],
    pub(super) tile_mercator_coords: [f32; 4],
    pub(super) dem_unpack: [f32; 4],
    pub(super) dem_dim: f32,
    pub(super) exaggeration: f32,
    pub(super) edge_index: [u32; 4],
    pub(super) surface_color: [f32; 4],
    pub(super) globe_origin: [f32; 4],
    pub(super) globe_basis: Option<cgmath::Matrix4<f64>>,
    pub(super) procedural_relief: [f32; 4],
    pub(super) procedural_grid: [f32; 4],
    pub(super) last_used_frame: u64,
}

pub(super) enum TileStaticRef<'a> {
    Cached(&'a CachedTileStatic),
    Bypass(Box<CachedTileStatic>),
}

impl std::ops::Deref for TileStaticRef<'_> {
    type Target = CachedTileStatic;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Cached(entry) => entry,
            Self::Bypass(entry) => entry,
        }
    }
}

#[derive(Default)]
pub(super) struct TileStaticUniformCache {
    frame_key: Option<StaticFrameKey>,
    frame_tick: u64,
    entries: FxHashMap<StaticTileKey, CachedTileStatic>,
}

impl TileStaticUniformCache {
    pub(super) fn begin_frame(&mut self, key: StaticFrameKey) {
        self.frame_tick = self.frame_tick.wrapping_add(1);
        if self.frame_key != Some(key) {
            self.frame_key = Some(key);
            self.entries.clear();
        }
    }

    pub(super) fn get_or_try_insert_with(
        &mut self,
        key: StaticTileKey,
        build: impl FnOnce() -> Option<CachedTileStatic>,
    ) -> Option<TileStaticRef<'_>> {
        let tick = self.frame_tick;
        if !self.entries.contains_key(&key) {
            let mut built = build()?;
            if self.entries.len() >= MAX_STATIC_CACHE_ENTRIES {
                self.entries
                    .retain(|_, value| tick.saturating_sub(value.last_used_frame) <= 4);
                if self.entries.len() >= MAX_STATIC_CACHE_ENTRIES {
                    self.entries
                        .retain(|_, value| value.last_used_frame == tick);
                }
                if self.entries.len() >= MAX_STATIC_CACHE_ENTRIES {
                    return Some(TileStaticRef::Bypass(Box::new(built)));
                }
            }
            built.last_used_frame = tick;
            self.entries.insert(key, built);
        }
        let entry = self.entries.get_mut(&key)?;
        entry.last_used_frame = tick;
        Some(TileStaticRef::Cached(entry))
    }
}

#[derive(Default)]
pub(super) struct TerrainQueueScratch {
    pub(super) prepared: PreparedTerrain,
    pub(super) static_cache: TileStaticUniformCache,
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
pub(super) struct AcquireScratch {
    pub(super) target_coords: Vec<WorldTileCoords>,
    pub(super) hidden: Vec<bool>,
    pub(super) undrawn: rustc_hash::FxHashSet<WorldTileCoords>,
}

#[derive(Default)]
pub(super) struct DrapeCoalesceState {
    moving_defer_streak: u8,
    pub(super) scratch: AcquireScratch,
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
    use super::{
        CachedTileStatic, DrapeCoalesceState, StaticFrameKey, StaticTileKey, TileStaticRef,
        TileStaticUniformCache, MAX_STATIC_CACHE_ENTRIES,
    };
    use crate::coords::{WorldTileCoords, ZoomLevel};

    fn dummy_frame_key() -> StaticFrameKey {
        StaticFrameKey {
            style_rev: 1,
            paint_zoom_bits: 20.0_f64.to_bits(),
            clear_color_bits: [0; 4],
            poles_north_bits: 0,
            poles_south_bits: 0,
            drape_mode_bits: (0, 1024, 0, 0, 0, 0),
            dem_exaggeration_bits: 1.0_f32.to_bits(),
            dem_unpack_bits: [0; 4],
            radius_bits: 6_371_008.8_f64.to_bits(),
            has_globe_camera: true,
            vector_arrival_epoch: 1,
            vector_tiles_len: 2,
        }
    }

    fn dummy_tile_key(idx: usize) -> StaticTileKey {
        let coords = WorldTileCoords {
            x: idx as i32,
            y: 0,
            z: ZoomLevel::new(22),
        };
        (coords, None, None, None)
    }

    fn dummy_static_tile(idx: usize) -> CachedTileStatic {
        CachedTileStatic {
            dem_matrix: [[0.0; 4]; 4],
            drape_matrix: [[0.0; 4]; 4],
            tile_mercator_coords: [idx as f32, 0.0, 0.0, 0.0],
            dem_unpack: [0.0; 4],
            dem_dim: 1.0,
            exaggeration: 1.0,
            edge_index: [0; 4],
            surface_color: [0.0; 4],
            globe_origin: [0.0; 4],
            globe_basis: None,
            procedural_relief: [0.0; 4],
            procedural_grid: [0.0; 4],
            last_used_frame: 0,
        }
    }

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

    #[test]
    fn static_uniform_cache_bypasses_overflow_without_dropping_tiles() {
        let mut cache = TileStaticUniformCache::default();
        let frame_key = dummy_frame_key();
        cache.begin_frame(frame_key);

        for i in 0..(MAX_STATIC_CACHE_ENTRIES + 32) {
            let entry =
                cache.get_or_try_insert_with(dummy_tile_key(i), || Some(dummy_static_tile(i)));
            if i < MAX_STATIC_CACHE_ENTRIES {
                assert!(
                    matches!(entry, Some(TileStaticRef::Cached(c)) if c.tile_mercator_coords[0] == i as f32)
                );
            } else {
                assert!(
                    matches!(entry, Some(TileStaticRef::Bypass(ref b)) if b.tile_mercator_coords[0] == i as f32)
                );
            }
        }
        assert_eq!(cache.entries.len(), MAX_STATIC_CACHE_ENTRIES);

        for _ in 0..6 {
            cache.begin_frame(frame_key);
        }
        let fresh_idx = MAX_STATIC_CACHE_ENTRIES + 100;
        let fresh = cache.get_or_try_insert_with(dummy_tile_key(fresh_idx), || {
            Some(dummy_static_tile(fresh_idx))
        });
        assert!(
            matches!(fresh, Some(TileStaticRef::Cached(c)) if c.tile_mercator_coords[0] == fresh_idx as f32)
        );
        assert_eq!(cache.entries.len(), 1);
    }
}

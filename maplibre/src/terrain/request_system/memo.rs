//! Per-frame memoization for DEM tile requests during stationary views and smooth flights.

use std::hash::{Hash, Hasher};

use rustc_hash::{FxHashSet, FxHasher};

use super::{DrapePrefetchRequests, DrapeRequests, TerrainWantedDems};
use crate::{
    coords::{ViewRegion, WorldTileCoords},
    io::{tile_backpressure::has_dem_deferred, tile_retry, tile_retry::RequestKind},
    render::{
        eye_covering::{frame_view_proj_bits, terrain_index_key, MultiViewExtractCovering},
        view_state::ViewState,
    },
    style::Style,
    tcs::world::World,
    terrain::coverage::{PreparedTerrainKey, PreparedTerrainRegion},
};

#[derive(Clone, PartialEq, Eq)]
pub(super) struct DemRequestKey {
    style_rev: u64,
    view_proj_bits: [[u64; 4]; 4],
    viewport_bits: (u64, u64),
    zoom_bits: u64,
    terrain_key: (u64, u64, usize),
    drapes_hash: u64,
    prefetch_hash: u64,
    dem_arrival_epoch: u64,
    tiles_len: usize,
    prepared_key: Option<PreparedTerrainKey>,
    multi_key: Option<u64>,
}

#[derive(Default)]
pub(super) struct DemRequestMemo {
    last_key: Option<DemRequestKey>,
    pub(super) last_region: Option<(PreparedTerrainKey, ViewRegion)>,
}

pub(super) fn hash_coords(coords: &[WorldTileCoords]) -> u64 {
    let mut hasher = FxHasher::default();
    coords.len().hash(&mut hasher);
    for c in coords {
        c.hash(&mut hasher);
    }
    hasher.finish()
}

pub(super) fn hash_multi_dem(multi: &MultiViewExtractCovering) -> u64 {
    let mut hasher = FxHasher::default();
    hash_coords(&multi.terrain_tiles).hash(&mut hasher);
    multi.skipped_wanted_dem.len().hash(&mut hasher);
    let mut skipped_mix = 0_u64;
    for c in &multi.skipped_wanted_dem {
        let mut elem = FxHasher::default();
        c.hash(&mut elem);
        skipped_mix ^= elem.finish();
    }
    skipped_mix.hash(&mut hasher);
    hasher.finish()
}

impl DemRequestMemo {
    pub(super) fn build_key(
        world: &World,
        style: &Style,
        view_state: &ViewState,
        dem_tile_size: u32,
        multi: Option<&MultiViewExtractCovering>,
    ) -> DemRequestKey {
        let drapes_hash = world
            .resources
            .get::<DrapeRequests>()
            .map_or(0, |r| hash_coords(&r.0));
        let prefetch_hash = world
            .resources
            .get::<DrapePrefetchRequests>()
            .map_or(0, |r| hash_coords(&r.0));
        let prepared_key = if multi.is_none() {
            Some(PreparedTerrainKey::capture(
                view_state,
                world,
                dem_tile_size,
            ))
        } else {
            None
        };
        DemRequestKey {
            style_rev: style.revision(),
            view_proj_bits: frame_view_proj_bits(world, style, view_state),
            viewport_bits: (view_state.width().to_bits(), view_state.height().to_bits()),
            zoom_bits: view_state.zoom().value().to_bits(),
            terrain_key: terrain_index_key(world),
            drapes_hash,
            prefetch_hash,
            dem_arrival_epoch: crate::terrain::dem_arrival_epoch(world),
            tiles_len: world.tiles.tiles.len(),
            prepared_key,
            multi_key: multi.map(hash_multi_dem),
        }
    }

    pub(super) fn try_reuse(&self, world: &mut World, key: &DemRequestKey) -> bool {
        if self.last_key.as_ref() != Some(key)
            || has_dem_deferred(world)
            || tile_retry::kind_has_due_retry(world, RequestKind::Dem)
            || world.resources.get::<TerrainWantedDems>().is_none()
        {
            return false;
        }
        if let Some(expected_key) = key.prepared_key {
            let matches = world
                .resources
                .get::<PreparedTerrainRegion>()
                .is_some_and(|prepared| prepared.key == expected_key);
            if !matches {
                let Some((cached_key, region)) = self.last_region.as_ref() else {
                    return false;
                };
                if *cached_key != expected_key {
                    return false;
                }
                world.resources.insert(PreparedTerrainRegion {
                    key: *cached_key,
                    region: Some(region.clone()),
                });
            }
        }
        true
    }

    pub(super) fn invalidate(&mut self) {
        self.last_key = None;
    }

    pub(super) fn record(
        &mut self,
        mut key: DemRequestKey,
        tiles_len: usize,
        region: Option<(PreparedTerrainKey, ViewRegion)>,
    ) {
        key.tiles_len = tiles_len;
        self.last_key = Some(key);
        if region.is_some() {
            self.last_region = region;
        }
    }
}

/// Inserts `requested` into `TerrainWantedDems` and bumps `TerrainWantedDemsEpoch` only when set membership changed.
pub(super) fn store_wanted_dems(world: &mut World, requested: FxHashSet<WorldTileCoords>) {
    let unchanged = world
        .resources
        .get::<TerrainWantedDems>()
        .is_some_and(|prev| prev.0 == requested);
    if !unchanged {
        let epoch = world
            .resources
            .get_or_init_mut::<super::TerrainWantedDemsEpoch>();
        epoch.0 = epoch.0.wrapping_add(1);
        world.resources.insert(TerrainWantedDems(requested));
    }
}

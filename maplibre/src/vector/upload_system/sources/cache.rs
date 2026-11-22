//! Frame cache for resolved vector source and paint tile sets.
use std::sync::Arc;

use super::{FrameSourcesAndPainted, FreshTightTiles, VectorFallbackTiles};
use crate::{
    coords::WorldTileCoords,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        view_state::{ViewState, ViewStatePadding},
    },
    style::Style,
    tcs::world::World,
    terrain::request_system::DrapeRequests,
};

#[derive(Clone, PartialEq, Eq)]
pub(super) struct FrameSourcesKey {
    view_proj_bits: [[u64; 4]; 4],
    zoom_bits: u64,
    style_zoom_bits: u64,
    viewport_bits: (u64, u64),
    external_view: bool,
    style_rev: u64,
    arrival_epoch: u64,
    pool_rev: u64,
    pool_evict: u64,
    tiles_len: usize,
    terrain_active: bool,
    pub(super) terrain_settled: bool,
    retained_drapes_len: usize,
    drape_hash: u64,
    fallback_hash: u64,
}

#[derive(Default)]
pub(super) struct FrameSourcesCache {
    pub(super) key: Option<FrameSourcesKey>,
    pub(super) result: Option<FrameSourcesAndPainted>,
    pub(super) fresh_tight: Option<FreshTightTiles>,
}

fn hash_coord_slice(coords: &[WorldTileCoords]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = rustc_hash::FxHasher::default();
    coords.len().hash(&mut hasher);
    for c in coords {
        c.hash(&mut hasher);
    }
    hasher.finish()
}

pub(super) fn build_frame_sources_key(
    world: &World,
    style: &Style,
    view_state: &ViewState,
) -> Option<FrameSourcesKey> {
    if world
        .resources
        .get::<crate::render::eye_covering::MultiViewExtractCovering>()
        .is_some_and(|m| m.active)
    {
        return None;
    }
    let Some(Initialized(pool)) = world
        .resources
        .get::<Eventually<crate::vector::VectorBufferPool>>()
    else {
        return None;
    };
    let terrain_active = style.terrain.is_some();
    let terrain_settled = !terrain_active
        || (world
            .resources
            .get::<crate::terrain::DrapePhase>()
            .is_none_or(|p| p.targets.is_empty())
            && !world
                .resources
                .get::<crate::terrain::queue_system::PendingDrapes>()
                .is_some_and(crate::terrain::queue_system::PendingDrapes::any_pending));
    let drape_hash = world
        .resources
        .get::<DrapeRequests>()
        .map_or(0, |r| hash_coord_slice(&r.0));
    let fallback_hash = world
        .resources
        .get::<VectorFallbackTiles>()
        .map_or(0, |f| hash_coord_slice(&f.0));
    let retained_drapes_len = world
        .resources
        .get::<crate::render::eye_covering::SlotResidencyFloor>()
        .map_or(0, |f| f.retained_drapes.len());
    Some(FrameSourcesKey {
        view_proj_bits: crate::render::eye_covering::frame_view_proj_bits(world, style, view_state),
        zoom_bits: view_state.zoom().value().to_bits(),
        style_zoom_bits: view_state.style_zoom().value().to_bits(),
        viewport_bits: (view_state.width().to_bits(), view_state.height().to_bits()),
        external_view: view_state.has_external_view(),
        style_rev: style.revision(),
        arrival_epoch: crate::vector::arrival_epoch(world),
        pool_rev: pool.revision(),
        pool_evict: pool.eviction_generation(),
        tiles_len: world.tiles.tiles.len(),
        terrain_active,
        terrain_settled,
        retained_drapes_len,
        drape_hash,
        fallback_hash,
    })
}

pub(super) fn update_loose_tight_tiles(world: &mut World, fresh_tight: Option<&FreshTightTiles>) {
    if let Some((level, tiles)) = fresh_tight {
        let host_frame = crate::render::frame_signals::host_frame_in_progress(world);
        let cache = world
            .resources
            .get_or_init_mut::<crate::render::eye_covering::FrameLooseCovering>();
        if cache.host_frame != host_frame {
            *cache = crate::render::eye_covering::FrameLooseCovering::for_host_frame(host_frame);
        }
        cache.tight_view_tiles = Some((Some(Arc::clone(tiles)), ViewStatePadding::Tight, *level));
    }
}

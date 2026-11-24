//! Frame cache for resolved vector source and paint tile sets.
use rustc_hash::FxHashSet;

use super::{FrameUploadSources, FreshTightTiles, VectorFallbackTiles};
use crate::{
    coords::{WorldTileCoords, ZoomLevel},
    io::tile_sources::{clamp_to_max_zoom, source_max_zoom, TileKind},
    render::{
        eventually::{Eventually, Eventually::Initialized},
        tile_view_pattern::DEFAULT_TILE_SIZE,
        view_state::ViewState,
    },
    style::Style,
    tcs::world::World,
    terrain::{
        drape_targets::select_targets, queue_system::uniforms::has_present_ancestor,
        request_system::DrapeRequests, resources::TerrainResources,
    },
};

#[derive(Clone, PartialEq, Eq)]
pub(super) struct FrameSourcesKey {
    view_tiles_present: bool,
    view_tiles_hash: u64,
    zoom_level: ZoomLevel,
    style_rev: u64,
    arrival_epoch: u64,
    pool_rev: u64,
    pool_evict: u64,
    tiles_len: usize,
    terrain_active: bool,
    pub(super) terrain_settled: bool,
    retained_drapes_len: usize,
    retained_drapes_hash: u64,
    multi_active: bool,
    current_slot: Option<u32>,
    drape_view: Option<u32>,
    drape_hash: u64,
    fallback_hash: u64,
}

#[derive(Default)]
pub(super) struct FrameSourcesCache {
    pub(super) key: Option<FrameSourcesKey>,
    pub(super) result: Option<FrameUploadSources>,
    pub(super) view_tiles_scratch: Vec<WorldTileCoords>,
}

/// Digest of `coords` as a set. A covering lists its tiles nearest the camera's focus first, so
/// an orbit reorders an unchanged covering; the sources resolved from it stay the same set.
pub(super) fn hash_coord_set(coords: &[WorldTileCoords]) -> u64 {
    use std::hash::{Hash, Hasher};
    coords.iter().fold(coords.len() as u64, |digest, c| {
        let mut hasher = rustc_hash::FxHasher::default();
        c.hash(&mut hasher);
        digest.wrapping_add(hasher.finish())
    })
}

fn hash_retained_drapes(drapes: &FxHashSet<WorldTileCoords>) -> u64 {
    use std::hash::{Hash, Hasher};
    drapes.iter().fold(drapes.len() as u64, |digest, c| {
        let mut hasher = rustc_hash::FxHasher::default();
        c.hash(&mut hasher);
        digest.wrapping_add(hasher.finish())
    })
}

fn terrain_key_fields(world: &World, terrain_active: bool) -> (bool, u64, usize, u64) {
    if !terrain_active {
        return (true, 0, 0, 0);
    }
    let settled = world
        .resources
        .get::<crate::terrain::DrapePhase>()
        .is_none_or(|p| p.targets.is_empty())
        && !world
            .resources
            .get::<crate::terrain::queue_system::PendingDrapes>()
            .is_some_and(crate::terrain::queue_system::PendingDrapes::any_pending);
    let drape_hash = world
        .resources
        .get::<DrapeRequests>()
        .map_or(0, |r| hash_coord_set(&r.0));
    let (retained_len, retained_hash) = world
        .resources
        .get::<crate::render::eye_covering::SlotResidencyFloor>()
        .map_or((0, 0), |f| {
            (
                f.retained_drapes.len(),
                hash_retained_drapes(&f.retained_drapes),
            )
        });
    (settled, drape_hash, retained_len, retained_hash)
}

pub(super) fn build_frame_sources_key(
    world: &World,
    style: &Style,
    view_state: &ViewState,
    view_tiles: Option<&[WorldTileCoords]>,
) -> Option<FrameSourcesKey> {
    let multi_active = world
        .resources
        .get::<crate::render::eye_covering::MultiViewExtractCovering>()
        .is_some_and(|m| m.active);
    let Some(Initialized(pool)) = world
        .resources
        .get::<Eventually<crate::vector::VectorBufferPool>>()
    else {
        return None;
    };
    let terrain_active = style.terrain.is_some();
    let (terrain_settled, drape_hash, retained_drapes_len, retained_drapes_hash) =
        terrain_key_fields(world, terrain_active);
    let fallback_hash = world
        .resources
        .get::<VectorFallbackTiles>()
        .map_or(0, |f| hash_coord_set(&f.0));
    let slot_raw = |s: &Option<crate::render::eye_covering::ViewSlotId>| {
        s.map(crate::render::eye_covering::ViewSlotId::as_u32)
    };
    Some(FrameSourcesKey {
        view_tiles_present: view_tiles.is_some(),
        view_tiles_hash: view_tiles.map_or(0, hash_coord_set),
        zoom_level: view_state.zoom().zoom_level(DEFAULT_TILE_SIZE),
        style_rev: style.revision(),
        arrival_epoch: crate::vector::arrival_epoch(world),
        pool_rev: pool.revision(),
        pool_evict: pool.eviction_generation(),
        tiles_len: world.tiles.tiles.len(),
        terrain_active,
        terrain_settled,
        retained_drapes_len,
        retained_drapes_hash,
        multi_active,
        current_slot: world
            .resources
            .get::<crate::render::eye_covering::CurrentViewSlot>()
            .and_then(|s| slot_raw(&s.0)),
        drape_view: world
            .resources
            .get::<crate::render::eye_covering::DrapeView>()
            .and_then(|v| slot_raw(&v.0)),
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
        cache.tight_view_tiles = Some((
            Some(std::sync::Arc::clone(tiles)),
            crate::render::view_state::ViewStatePadding::Tight,
            *level,
        ));
    }
}

pub(super) fn terrain_sources(
    world: &World,
    style: &Style,
) -> (Vec<WorldTileCoords>, FxHashSet<WorldTileCoords>) {
    let max_zoom = source_max_zoom(style, TileKind::Vector);
    let terrain = match world.resources.get::<Eventually<TerrainResources>>() {
        Some(Initialized(t)) => Some(t),
        _ => None,
    };
    let targets = std::iter::once(WorldTileCoords::default())
        .chain(
            world
                .resources
                .get::<DrapeRequests>()
                .into_iter()
                .flat_map(|requests| requests.0.iter().copied()),
        )
        .chain(
            world
                .resources
                .get::<crate::render::eye_covering::SlotResidencyFloor>()
                .into_iter()
                .flat_map(|floor| floor.retained_drapes.iter().copied()),
        );
    let selected = select_targets(targets, world, &[]);
    let mut seen = FxHashSet::default();
    let mut uncovered_list = Vec::new();
    let mut covered_list = Vec::new();
    let mut uncovered_set = FxHashSet::default();
    for (target, shapes) in selected {
        let is_uncovered = terrain
            .is_some_and(|t| t.drape_texture(target).is_none() && !has_present_ancestor(target, t));
        for source in shapes
            .into_iter()
            .filter(|shape| shape.raster_source.is_none())
            .map(|shape| shape.coords)
            .chain(std::iter::once(clamp_to_max_zoom(target, max_zoom)))
        {
            if is_uncovered {
                uncovered_set.insert(source);
            }
            if seen.insert(source) {
                if is_uncovered {
                    uncovered_list.push(source);
                } else {
                    covered_list.push(source);
                }
            }
        }
    }
    uncovered_list.extend(covered_list);
    (uncovered_list, uncovered_set)
}

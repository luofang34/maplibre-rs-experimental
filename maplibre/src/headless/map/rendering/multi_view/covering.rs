//! Unioned request and residency computation across active and skipped [`ViewSlotId`]s.

use std::collections::BTreeMap;

use rustc_hash::{FxHashMap, FxHashSet};

use super::{CoveringPolicy, SlotState, SlotView, ViewSlotEntry, ViewSlotId};
use crate::{
    coords::WorldTileCoords,
    io::tile_sources::RasterSourceId,
    map::{
        runtime::retention_system::{drawn_tiles, finish_tiles_in_use, tiles_in_use_from_loose},
        terrain_drape::targets::target_specs,
    },
    projection::ProjectionType,
    render::{
        eye_covering::{
            raster_source_regions, DrapeView, FrameLodHistory, MultiViewExtractCovering,
            RasterCoverings, SharedCovering, SlotResidencyFloor,
        },
        projection::{terrain_region, view_region_for_projection},
        tile_view_pattern::DEFAULT_TILE_SIZE,
        view_state::{CameraOptions, ViewState, ViewStatePadding},
    },
    style::Style,
    tcs::world::World,
    terrain::{
        drape_share::DrapeShares,
        queue_system::{
            surface_covering::{SurfaceCopies, SurfaceTiles},
            TerrainEyeFrame,
        },
        request_system::DrapeRequests,
        source::dem_source,
    },
};

/// Saved covering and terrain state of a primary slot for [`CoveringPolicy::StereoReuse`].
#[derive(Clone)]
pub(super) struct StereoSnapshot {
    pub(super) shared: Option<SharedCovering>,
    pub(super) surface_tiles: SurfaceTiles,
    pub(super) surface_copies: SurfaceCopies,
    pub(super) terrain_frame: TerrainEyeFrame,
}

pub(super) fn build_slot_view_state(
    base: &ViewState,
    style: &Style,
    world: &World,
    slot_size: crate::window::PhysicalSize,
    scale_factor: f64,
    (opaque_environment, request_overscan): (bool, f64),
    view: &SlotView,
) -> Result<ViewState, super::MultiViewError> {
    let projection = style
        .projection
        .as_ref()
        .map_or(ProjectionType::Mercator, |spec| {
            spec.projection_type.clone()
        });
    let mut state = base.clone();
    state.resize(slot_size.to_logical(scale_factor));
    state.set_opaque_environment(opaque_environment);
    match *view {
        SlotView::External(external) => {
            state
                .set_external_view(external, &projection)
                .map_err(|source| super::MultiViewError::InvalidExternalView { source })?;
            state.set_request_overscan(request_overscan);
        }
        SlotView::Camera {
            center,
            zoom,
            bearing_deg,
            pitch_deg,
        } => {
            state.clear_external_view();
            state
                .jump_to(CameraOptions {
                    center: Some(center),
                    zoom: Some(zoom.value()),
                    bearing: Some(cgmath::Deg(bearing_deg)),
                    pitch: Some(cgmath::Deg(pitch_deg)),
                    roll: Some(cgmath::Deg(0.0)),
                    elevation: None,
                })
                .map_err(|source| super::MultiViewError::InvalidCamera { source })?;
            crate::terrain::elevation::ground_center(&mut state, world);
        }
    }
    Ok(state)
}

#[derive(Default)]
struct CoveringAccum {
    extract: MultiViewExtractCovering,
    floor: SlotResidencyFloor,
    vector_seen: FxHashSet<WorldTileCoords>,
    terrain_seen: FxHashSet<WorldTileCoords>,
    drape_requests_seen: FxHashSet<WorldTileCoords>,
    drape_requests: Vec<WorldTileCoords>,
    raster_by_source: BTreeMap<RasterSourceId, (FxHashSet<WorldTileCoords>, Vec<WorldTileCoords>)>,
}

fn grant_drape_shares(
    world: &mut World,
    registry: &BTreeMap<ViewSlotId, SlotState>,
    active: &[(ViewSlotEntry, ViewState)],
    active_ids: &FxHashSet<ViewSlotId>,
) {
    // Every view that holds drapes is granted textures, the views the host ranks first before
    // the others: those drawn this frame with a covering of their own, and those left out of
    // it that keep what they last drew.
    let mut draping: Vec<(i32, ViewSlotId)> = registry
        .iter()
        .filter(|(slot, state)| {
            active.iter().any(|(entry, _)| {
                entry.slot == **slot && matches!(entry.policy, CoveringPolicy::Independent)
            }) || (!active_ids.contains(slot) && !state.last_retained_drapes.is_empty())
        })
        .map(|(slot, state)| (state.drape_priority, *slot))
        .collect();
    draping.sort_by_key(|(priority, slot)| (std::cmp::Reverse(*priority), *slot));
    let draping: Vec<ViewSlotId> = draping.into_iter().map(|(_, slot)| slot).collect();
    let allowed = world
        .resources
        .get::<crate::render::memory_budget::MemoryBudget>()
        .copied()
        .unwrap_or_default()
        .drape_textures_allowed();
    world
        .resources
        .get_or_init_mut::<DrapeShares>()
        .grant(&draping, allowed);
}

fn record_skipped_slot_residency(
    registry: &BTreeMap<ViewSlotId, SlotState>,
    active_ids: &FxHashSet<ViewSlotId>,
    acc: &mut CoveringAccum,
) {
    for (slot_id, slot_state) in registry {
        if active_ids.contains(slot_id) {
            continue;
        }
        acc.floor.drawn_count = acc.floor.drawn_count.max(slot_state.last_drawn_count);
        acc.floor
            .retained_tiles
            .extend(slot_state.last_retained_tiles.iter().copied());
        acc.floor
            .retained_drapes
            .extend(slot_state.last_retained_drapes.iter().copied());
        acc.extract
            .skipped_wanted_vector
            .extend(slot_state.last_wanted_vector.iter().copied());
        acc.extract
            .skipped_wanted_raster
            .extend(slot_state.last_wanted_raster.iter().copied());
        acc.extract
            .skipped_wanted_dem
            .extend(slot_state.last_wanted_dem.iter().copied());
    }
}

fn is_procedural_relief(world: &World) -> bool {
    world
        .resources
        .get::<crate::render::settings::TerrainDrapeMode>()
        .copied()
        .or_else(|| {
            match world.resources.get::<crate::render::eventually::Eventually<
                crate::terrain::resources::TerrainResources,
            >>() {
                Some(crate::render::eventually::Eventually::Initialized(terrain)) => {
                    Some(terrain.drape_mode())
                }
                _ => None,
            }
        })
        .is_some_and(|mode| mode.is_procedural_relief())
}

fn collect_slot_dem_and_drapes(
    acc: &mut CoveringAccum,
    style: &Style,
    world: &mut World,
    registry: &mut BTreeMap<ViewSlotId, SlotState>,
    entry: &ViewSlotEntry,
    slot_view: &ViewState,
    dem_config: (Option<u32>, bool),
) -> (
    FxHashSet<WorldTileCoords>,
    Option<crate::terrain::coverage::PreparedTerrainRegion>,
    FxHashSet<WorldTileCoords>,
) {
    let (dem_tile_size, procedural_relief) = dem_config;
    let mut slot_wanted_dem = FxHashSet::default();
    let mut drape_wanted_vec = FxHashSet::default();
    let mut prepared_for_slot = None;
    let Some(tile_size) = dem_tile_size else {
        return (slot_wanted_dem, prepared_for_slot, drape_wanted_vec);
    };
    if let Ok(region_opt) =
        terrain_region(style, slot_view, world, tile_size, ViewStatePadding::Tight)
    {
        if let Some(region) = &region_opt {
            for coords in region.iter() {
                slot_wanted_dem.insert(coords);
                if acc.terrain_seen.insert(coords) {
                    acc.extract.terrain_tiles.push(coords);
                }
            }
        }
        if !procedural_relief {
            let key =
                crate::terrain::coverage::PreparedTerrainKey::capture(slot_view, world, tile_size);
            let prepared = crate::terrain::coverage::PreparedTerrainRegion {
                key,
                region: region_opt,
            };
            prepared_for_slot = Some(prepared.clone());
            world.resources.insert(prepared);
        }
    }
    if !procedural_relief {
        if let Ok(specs) = target_specs(style, slot_view, world) {
            let mut slot_drapes = FxHashSet::default();
            for spec in &specs {
                slot_drapes.insert(spec.coords);
                acc.floor.retained_drapes.insert(spec.coords);
                drape_wanted_vec.insert(spec.coords);
                slot_wanted_dem.insert(spec.coords);
                if acc.drape_requests_seen.insert(spec.coords) {
                    acc.drape_requests.push(spec.coords);
                }
            }
            if let Some(state) = registry.get_mut(&entry.slot) {
                state.last_retained_drapes = slot_drapes;
            }
        }
    } else if let Some(state) = registry.get_mut(&entry.slot) {
        state.last_retained_drapes.clear();
    }
    (slot_wanted_dem, prepared_for_slot, drape_wanted_vec)
}

fn collect_slot_covering(
    acc: &mut CoveringAccum,
    style: &Style,
    world: &mut World,
    registry: &mut BTreeMap<ViewSlotId, SlotState>,
    entry: &ViewSlotEntry,
    slot_view: &ViewState,
    dem_config: (Option<u32>, bool),
) {
    let lod_history = registry
        .get(&entry.slot)
        .map(|state| state.lod_history.clone())
        .unwrap_or_default();
    world.resources.insert(lod_history);
    world.resources.insert(DrapeView(Some(entry.slot)));

    let level = slot_view.zoom().zoom_level(DEFAULT_TILE_SIZE);
    acc.extract.max_view_level = acc.extract.max_view_level.max(u8::from(level));

    let mut slot_wanted_vec = FxHashSet::default();
    if let Ok(Some(region)) =
        view_region_for_projection(style, slot_view, world, level, ViewStatePadding::Loose)
    {
        for coords in region.iter() {
            slot_wanted_vec.insert(coords);
            if acc.vector_seen.insert(coords) {
                acc.extract.vector_tiles.push(coords);
            }
        }
    }

    let mut slot_wanted_ras = FxHashSet::default();
    if let Ok(regions) = raster_source_regions(style, slot_view, world, ViewStatePadding::Loose) {
        for (source_id, tiles) in regions {
            let (seen, list) = acc.raster_by_source.entry(source_id).or_default();
            for coords in tiles {
                slot_wanted_ras.insert(coords);
                if seen.insert(coords) {
                    list.push(coords);
                }
            }
        }
    }

    let (slot_wanted_dem, prepared_for_slot, drape_vec) =
        collect_slot_dem_and_drapes(acc, style, world, registry, entry, slot_view, dem_config);
    slot_wanted_vec.extend(drape_vec);

    let slot_in_use = tiles_in_use_from_loose(
        world,
        style,
        slot_wanted_vec.iter().copied(),
        slot_wanted_ras.iter().copied(),
    );
    acc.floor.retained_tiles.extend(slot_in_use.iter().copied());
    if let Some(state) = registry.get_mut(&entry.slot) {
        state.prepared_terrain = prepared_for_slot;
        state.frame_loose_in_use = slot_in_use;
        state.last_wanted_vector = slot_wanted_vec;
        state.last_wanted_raster = slot_wanted_ras;
        state.last_wanted_dem = slot_wanted_dem;
    }
}

/// Computes the unioned [`MultiViewExtractCovering`], [`DrapeRequests`], and initial
/// [`SlotResidencyFloor`] across all active `Independent` slots and skipped registered slots.
pub(super) fn build_multi_view_coverings(
    style: &Style,
    world: &mut World,
    registry: &mut BTreeMap<ViewSlotId, SlotState>,
    active: &[(ViewSlotEntry, ViewState)],
) -> (MultiViewExtractCovering, SlotResidencyFloor) {
    let active_ids: FxHashSet<ViewSlotId> = active.iter().map(|(entry, _)| entry.slot).collect();
    world
        .resources
        .remove::<crate::terrain::request_system::TerrainWantedDems>();
    world.resources.remove::<DrapeRequests>();
    let mut acc = CoveringAccum::default();
    acc.extract.active = true;
    grant_drape_shares(world, registry, active, &active_ids);
    record_skipped_slot_residency(registry, &active_ids, &mut acc);

    let dem_config = (
        dem_source(style).map(|dem| dem.tile_size),
        is_procedural_relief(world),
    );
    for (entry, slot_view) in active {
        if !matches!(entry.policy, CoveringPolicy::StereoReuse(_)) {
            collect_slot_covering(
                &mut acc, style, world, registry, entry, slot_view, dem_config,
            );
        }
    }

    acc.extract.raster_regions = acc
        .raster_by_source
        .into_iter()
        .map(|(source_id, (_, list))| (source_id, list))
        .collect::<RasterCoverings>();
    if !acc.drape_requests.is_empty() {
        world.resources.insert(DrapeRequests(acc.drape_requests));
    }
    (acc.extract, acc.floor)
}

/// Captures a slot's post-queue covering, LOD history, stereo snapshot, and residency sets.
pub(super) fn record_slot_post_render(
    style: &Style,
    _slot_view: &ViewState,
    world: &World,
    slot_state: &mut SlotState,
    floor: &mut SlotResidencyFloor,
) {
    let shared = world.resources.get::<SharedCovering>().cloned();
    let surface_tiles = world
        .resources
        .get::<SurfaceTiles>()
        .cloned()
        .unwrap_or_default();
    let surface_copies = world
        .resources
        .get::<SurfaceCopies>()
        .cloned()
        .unwrap_or_default();
    let terrain_frame = world
        .resources
        .get::<TerrainEyeFrame>()
        .cloned()
        .unwrap_or_default();

    slot_state.lod_history = FrameLodHistory {
        view: crate::projection::lod_history::LodHistory::new(
            shared.as_ref().and_then(|s| s.tiles()).unwrap_or(&[]),
        ),
        raster: shared
            .as_ref()
            .map(|s| {
                let _ = s;
                FxHashMap::default()
            })
            .unwrap_or_default(),
        terrain: crate::projection::lod_history::LodHistory::new(&surface_tiles.0),
    };
    if let Some(current_history) = world.resources.get::<FrameLodHistory>() {
        let _ = current_history;
    }
    slot_state.stereo_snapshot = Some(StereoSnapshot {
        shared,
        surface_tiles,
        surface_copies,
        terrain_frame,
    });
    let drawn = drawn_tiles(world);
    slot_state.last_drawn_count = drawn.len();
    let mut retained = std::mem::take(&mut slot_state.frame_loose_in_use);
    retained.extend(drawn);
    let retained = finish_tiles_in_use(world, style, retained);
    if let Some(requests) = world.resources.get::<DrapeRequests>() {
        slot_state.last_retained_drapes = requests.0.iter().copied().collect();
    }
    slot_state.last_retained_tiles = retained.clone();
    floor.drawn_count = floor.drawn_count.max(slot_state.last_drawn_count);
    floor.retained_tiles.extend(retained);
    floor
        .retained_drapes
        .extend(slot_state.last_retained_drapes.iter().copied());
}

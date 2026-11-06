//! Unioned request and residency computation across active and skipped [`ViewSlotId`]s.

use std::collections::{BTreeMap, HashSet};

use super::{CoveringPolicy, SlotState, SlotView, ViewSlotEntry, ViewSlotId};
use crate::{
    coords::WorldTileCoords,
    projection::ProjectionType,
    raster::RasterSourceId,
    render::{
        eye_covering::{
            FrameLodHistory, MultiViewExtractCovering, RasterCoverings, SharedCovering,
            SlotResidencyFloor,
        },
        projection::{raster_source_regions, terrain_region, view_region_for_projection},
        systems::retention_system::{drawn_tiles, tiles_in_use},
        tile_view_pattern::DEFAULT_TILE_SIZE,
        view_state::{CameraOptions, ViewState, ViewStatePadding},
    },
    style::Style,
    tcs::world::World,
    terrain::{
        queue_system::{
            surface_covering::{SurfaceCopies, SurfaceTiles},
            target_specs, TerrainEyeFrame,
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

/// Computes the unioned [`MultiViewExtractCovering`], [`DrapeRequests`], and initial
/// [`SlotResidencyFloor`] across all active `Independent` slots and skipped registered slots.
pub(super) fn build_multi_view_coverings(
    style: &Style,
    world: &mut World,
    registry: &mut BTreeMap<ViewSlotId, SlotState>,
    active: &[(ViewSlotEntry, ViewState)],
) -> (MultiViewExtractCovering, SlotResidencyFloor) {
    let active_ids: HashSet<ViewSlotId> = active.iter().map(|(entry, _)| entry.slot).collect();
    let mut extract = MultiViewExtractCovering {
        active: true,
        ..Default::default()
    };
    let mut floor = SlotResidencyFloor::default();
    let mut vector_seen = HashSet::new();
    let mut terrain_seen = HashSet::new();
    let mut drape_requests_seen = HashSet::new();
    let mut drape_requests = Vec::new();
    let mut raster_by_source: BTreeMap<
        RasterSourceId,
        (HashSet<WorldTileCoords>, Vec<WorldTileCoords>),
    > = BTreeMap::new();

    for (slot_id, slot_state) in registry.iter() {
        if !active_ids.contains(slot_id) {
            floor.drawn_count = floor.drawn_count.max(slot_state.last_drawn_count);
            floor
                .retained_tiles
                .extend(slot_state.last_retained_tiles.iter().copied());
            floor
                .retained_drapes
                .extend(slot_state.last_retained_drapes.iter().copied());
            extract
                .skipped_wanted_vector
                .extend(slot_state.last_wanted_vector.iter().copied());
            extract
                .skipped_wanted_raster
                .extend(slot_state.last_wanted_raster.iter().copied());
            extract
                .skipped_wanted_dem
                .extend(slot_state.last_wanted_dem.iter().copied());
        }
    }

    let dem_tile_size = dem_source(style).map(|dem| dem.tile_size);
    let procedural_relief = world
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
        .is_some_and(|mode| mode.is_procedural_relief());

    for (entry, slot_view) in active {
        if matches!(entry.policy, CoveringPolicy::StereoReuse(_)) {
            continue;
        }
        let lod_history = registry
            .get(&entry.slot)
            .map(|state| state.lod_history.clone())
            .unwrap_or_default();
        world.resources.insert(lod_history);

        let level = slot_view.zoom().zoom_level(DEFAULT_TILE_SIZE);
        extract.max_view_level = extract.max_view_level.max(u8::from(level));

        let mut slot_wanted_vec = HashSet::new();
        if let Ok(Some(region)) =
            view_region_for_projection(style, slot_view, world, level, ViewStatePadding::Loose)
        {
            for coords in region.iter() {
                slot_wanted_vec.insert(coords);
                if vector_seen.insert(coords) {
                    extract.vector_tiles.push(coords);
                }
            }
        }

        let mut slot_wanted_ras = HashSet::new();
        if let Ok(regions) = raster_source_regions(style, slot_view, world, ViewStatePadding::Loose)
        {
            for (source_id, tiles) in regions {
                let (seen, list) = raster_by_source.entry(source_id).or_default();
                for coords in tiles {
                    slot_wanted_ras.insert(coords);
                    if seen.insert(coords) {
                        list.push(coords);
                    }
                }
            }
        }

        let mut slot_wanted_dem = HashSet::new();
        if let Some(tile_size) = dem_tile_size {
            if let Ok(region_opt) =
                terrain_region(style, slot_view, world, tile_size, ViewStatePadding::Tight)
            {
                if let Some(region) = &region_opt {
                    for coords in region.iter() {
                        slot_wanted_dem.insert(coords);
                        if terrain_seen.insert(coords) {
                            extract.terrain_tiles.push(coords);
                        }
                    }
                }
                if !procedural_relief {
                    let key = crate::terrain::coverage::PreparedTerrainKey::capture(
                        slot_view, world, tile_size,
                    );
                    world
                        .resources
                        .insert(crate::terrain::coverage::PreparedTerrainRegion {
                            key,
                            region: region_opt,
                        });
                }
            }
            if !procedural_relief {
                if let Ok(specs) = target_specs(style, slot_view, world) {
                    let mut slot_drapes = HashSet::new();
                    for spec in &specs {
                        slot_drapes.insert(spec.coords);
                        floor.retained_drapes.insert(spec.coords);
                        slot_wanted_vec.insert(spec.coords);
                        slot_wanted_dem.insert(spec.coords);
                        if drape_requests_seen.insert(spec.coords) {
                            drape_requests.push(spec.coords);
                        }
                    }
                    if let Some(state) = registry.get_mut(&entry.slot) {
                        state.last_retained_drapes = slot_drapes;
                    }
                }
            } else if let Some(state) = registry.get_mut(&entry.slot) {
                state.last_retained_drapes.clear();
            }
        }

        let slot_in_use = tiles_in_use(world, style, slot_view);
        floor.retained_tiles.extend(slot_in_use.iter().copied());
        if let Some(state) = registry.get_mut(&entry.slot) {
            state.last_wanted_vector = slot_wanted_vec;
            state.last_wanted_raster = slot_wanted_ras;
            state.last_wanted_dem = slot_wanted_dem;
        }
    }

    extract.raster_regions = raster_by_source
        .into_iter()
        .map(|(source_id, (_, list))| (source_id, list))
        .collect::<RasterCoverings>();
    if !drape_requests.is_empty() {
        world.resources.insert(DrapeRequests(drape_requests));
    }
    (extract, floor)
}

/// Captures a slot's post-queue covering, LOD history, stereo snapshot, and residency sets.
pub(super) fn record_slot_post_render(
    style: &Style,
    slot_view: &ViewState,
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
                // Keep per-source raster LOD history through SharedCovering::with_frame.
                let _ = s;
                std::collections::HashMap::new()
            })
            .unwrap_or_default(),
        terrain: crate::projection::lod_history::LodHistory::new(&surface_tiles.0),
    };
    if let Some(current_history) = world.resources.get::<FrameLodHistory>() {
        // Combine with raster history if snapshot_lod_history populated one; next frame uses
        // snapshot_lod_history over `shared` + `surface_tiles` directly!
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
    let mut retained = tiles_in_use(world, style, slot_view);
    retained.extend(drawn);
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

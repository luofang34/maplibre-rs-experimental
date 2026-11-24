//! Retains only the raster and DEM fallback ancestors actually needed by unsettled tiles.

use rustc_hash::FxHashSet;

use crate::{
    coords::WorldTileCoords,
    io::{
        tile_backpressure::is_settled,
        tile_sources::{RasterSourceId, TileKind},
    },
    render::{
        eventually::{Eventually, Eventually::Initialized},
        eye_covering::FrameLooseCovering,
        projection::view_region_for_projection,
        view_state::{ViewState, ViewStatePadding},
    },
    style::Style,
    tcs::{tiles::Tiles, world::World},
    terrain::{dem_tile_coords, resources::TerrainResources, source::dem_source},
};

pub(super) fn bump_arrival_epochs(world: &mut World) {
    crate::vector::bump_arrival_epoch(world);
    crate::terrain::bump_dem_arrival_epoch(world);
    #[cfg(feature = "raster")]
    crate::raster::bump_raster_arrival_epoch(world);
}

pub(super) fn retain_requested_vector_tiles(
    world: &World,
    style: &Style,
    view_state: &ViewState,
    (loose_cache, request_padding, max_zoom, visible_level): (
        Option<&FrameLooseCovering>,
        ViewStatePadding,
        Option<u8>,
        crate::coords::ZoomLevel,
    ),
    in_use: &mut FxHashSet<WorldTileCoords>,
) {
    if let Some((cached_view, _, _)) = loose_cache
        .and_then(|c| c.view_tiles.as_ref())
        .filter(|(_, padding, _)| *padding == request_padding)
    {
        for &coords in cached_view.as_deref().into_iter().flatten() {
            super::retain_loose_vector_tile(in_use, coords, max_zoom);
        }
        return;
    }
    if let Some(shared_tiles) = (request_padding == ViewStatePadding::Tight)
        .then(|| {
            crate::render::eye_covering::matching_tight_tiles(
                world,
                style,
                view_state,
                visible_level,
            )
        })
        .flatten()
    {
        for &coords in shared_tiles {
            super::retain_loose_vector_tile(in_use, coords, max_zoom);
        }
        return;
    }
    match view_region_for_projection(style, view_state, world, visible_level, request_padding) {
        Ok(Some(requested)) => {
            for coords in requested.iter() {
                super::retain_loose_vector_tile(in_use, coords, max_zoom);
            }
        }
        Ok(None) => {}
        Err(error) => tracing::warn!(%error, "cannot select the requested tiles to keep"),
    }
}

pub(super) fn retain_requested_raster_tiles(
    world: &World,
    style: &Style,
    view_state: &ViewState,
    (loose_cache, request_padding): (Option<&FrameLooseCovering>, ViewStatePadding),
    in_use: &mut FxHashSet<WorldTileCoords>,
) {
    if let Some((cached_raster, _, _)) = loose_cache
        .and_then(|c| c.raster_regions.as_ref())
        .filter(|(_, padding, _)| *padding == request_padding)
    {
        for (_, tiles) in cached_raster {
            in_use.extend(tiles.iter().copied());
        }
    } else if let Ok(regions) =
        crate::render::projection::raster_source_regions(style, view_state, world, request_padding)
    {
        for (_, tiles) in regions {
            in_use.extend(tiles);
        }
    }
}

/// Appends active drape requests, drape prefetch requests, terrain overview + wanted DEMs,
/// non-terrain vector fallback tiles, and only the raster/DEM fallback ancestors actually needed
/// by tiles whose own source data (and GPU DEM upload) has not yet settled.
pub(crate) fn finish_tiles_in_use(
    world: &World,
    style: &Style,
    mut in_use: FxHashSet<WorldTileCoords>,
) -> FxHashSet<WorldTileCoords> {
    let dem = dem_source(style);
    let max_zoom = crate::io::tile_sources::source_max_zoom(style, TileKind::Vector);
    if style.terrain.is_some()
        && style.cached_has_vector_source_layers()
        && crate::io::tile_sources::source_min_zoom(style, TileKind::Vector)
            .is_none_or(|min| min == 0)
    {
        in_use.insert(WorldTileCoords::default());
    }
    if let Some(requests) = world
        .resources
        .get::<crate::terrain::request_system::DrapeRequests>()
    {
        for &coords in &requests.0 {
            retain_drape_tile(&mut in_use, coords, max_zoom, dem.as_ref());
        }
    }
    if let Some(prefetch) = world
        .resources
        .get::<crate::terrain::request_system::DrapePrefetchRequests>()
    {
        for &coords in &prefetch.0 {
            retain_drape_tile(&mut in_use, coords, max_zoom, dem.as_ref());
        }
    }
    if let Some(wanted_dems) = world
        .resources
        .get::<crate::terrain::request_system::TerrainWantedDems>()
    {
        in_use.extend(wanted_dems.0.iter().copied());
    }
    if let Some(fallbacks) = world
        .resources
        .get::<crate::vector::upload_system::sources::VectorFallbackTiles>()
    {
        in_use.extend(fallbacks.0.iter().copied());
    }
    let raster_sources = active_raster_sources(style);
    if dem.is_none() && raster_sources.is_empty() {
        return in_use;
    }
    let terrain_gpu = match world.resources.get::<Eventually<TerrainResources>>() {
        Some(Initialized(terrain)) => Some(terrain),
        _ => None,
    };
    let view: Vec<WorldTileCoords> = in_use.iter().copied().collect();
    for coords in view {
        for (source_id, minzoom) in &raster_sources {
            retain_raster_fallback_ancestors(
                &world.tiles,
                &mut in_use,
                coords,
                source_id,
                *minzoom,
                dem.is_some(),
            );
        }
        if let Some(dem) = &dem {
            retain_dem_fallback_ancestors(&world.tiles, terrain_gpu, &mut in_use, coords, dem);
        }
    }
    in_use
}

fn retain_drape_tile(
    in_use: &mut FxHashSet<WorldTileCoords>,
    coords: WorldTileCoords,
    max_zoom: Option<u8>,
    dem: Option<&crate::terrain::source::DemSource>,
) {
    if let Some((canonical, _)) = coords.wrapped() {
        in_use.insert(canonical);
    } else {
        in_use.insert(coords);
    }
    if let Some(target) =
        crate::coords::OverscaledTileId::for_source(coords, u8::from(coords.z), max_zoom, false)
    {
        in_use.insert(target.canonical);
    }
    if let Some(dem) = dem {
        if let Some(dem_coords) = dem_tile_coords(coords, dem.minzoom, dem.maxzoom) {
            in_use.insert(dem_coords);
        }
    }
}

pub(super) fn retain_terrain_region_if_needed(
    world: &World,
    style: &Style,
    view_state: &crate::render::view_state::ViewState,
    padding: crate::render::view_state::ViewStatePadding,
    in_use: &mut FxHashSet<WorldTileCoords>,
) {
    if world
        .resources
        .get::<crate::terrain::request_system::TerrainWantedDems>()
        .is_some()
    {
        return;
    }
    let Some(dem) = dem_source(style) else {
        return;
    };
    let Ok(Some(region)) =
        crate::render::projection::terrain_region(style, view_state, world, dem.tile_size, padding)
    else {
        return;
    };
    for coords in region.iter() {
        if let Some(dem_coords) = dem_tile_coords(coords, dem.minzoom, dem.maxzoom) {
            in_use.insert(dem_coords);
        }
    }
}

fn active_raster_sources(style: &Style) -> Vec<(RasterSourceId, u8)> {
    if !style.cached_has_any_raster_type(&crate::io::tile_sources::RASTER_LAYER_TYPES) {
        return Vec::new();
    }
    let mut seen = FxHashSet::default();
    let mut out = Vec::new();
    for group in crate::io::tile_sources::source_layer_groups(style, TileKind::Raster) {
        let minzoom = match group
            .source_name
            .as_deref()
            .and_then(|name| style.sources.get(name))
        {
            Some(crate::style::source::Source::Raster(s)) => s.minzoom.unwrap_or(0),
            Some(crate::style::source::Source::RasterDem(s)) => s.minzoom.unwrap_or(0),
            _ => 0,
        };
        let id = RasterSourceId::new(group.source_name);
        if seen.insert(id.clone()) {
            out.push((id, minzoom));
        }
    }
    out
}

fn tile_has_raster_data(tiles: &Tiles, source: &RasterSourceId, coords: WorldTileCoords) -> bool {
    #[cfg(feature = "raster")]
    let has_raster = tiles
        .query::<&crate::raster::RasterLayersDataComponent>(coords)
        .is_some_and(|c| {
            c.layers.iter().any(|layer| {
                matches!(
                    layer,
                    crate::raster::RasterLayerData::Available(avail) if &avail.source == source
                ) || matches!(
                    layer,
                    crate::raster::RasterLayerData::Empty(empty) if &empty.source == source
                )
            })
        });
    #[cfg(not(feature = "raster"))]
    let has_raster = false;
    has_raster
        || tiles
            .query::<&crate::terrain::DemLayersComponent>(coords)
            .is_some_and(|c| c.contains(source))
}

fn tile_raster_settled(tiles: &Tiles, source: &RasterSourceId, coords: WorldTileCoords) -> bool {
    #[cfg(feature = "raster")]
    let settled = tiles
        .query::<&crate::raster::RasterLayersDataComponent>(coords)
        .is_some_and(|c| c.has_source_result(source));
    #[cfg(not(feature = "raster"))]
    let settled = false;
    settled
        || tiles
            .query::<&crate::terrain::DemLayersComponent>(coords)
            .is_some_and(|c| c.contains(source))
}

fn retain_raster_fallback_ancestors(
    tiles: &Tiles,
    in_use: &mut FxHashSet<WorldTileCoords>,
    coords: WorldTileCoords,
    source: &RasterSourceId,
    minzoom: u8,
    is_terrain: bool,
) {
    if tile_has_raster_data(tiles, source, coords) {
        return;
    }
    if is_terrain && !tile_raster_settled(tiles, source, coords) {
        let target_coarse = u8::from(coords.z).saturating_sub(3).max(minzoom);
        if target_coarse < u8::from(coords.z) {
            let mut coarse = coords;
            while u8::from(coarse.z) > target_coarse {
                let Some(parent) = coarse.get_parent() else {
                    break;
                };
                coarse = parent;
            }
            if u8::from(coarse.z) == target_coarse {
                in_use.insert(coarse);
            }
        }
    }
    let mut current = coords;
    while let Some(parent) = current.get_parent() {
        if u8::from(parent.z) < minzoom {
            break;
        }
        if tile_has_raster_data(tiles, source, parent) {
            in_use.insert(parent);
            break;
        }
        if !is_settled(tiles, parent) {
            in_use.insert(parent);
        }
        current = parent;
    }
}

fn retain_dem_fallback_ancestors(
    tiles: &Tiles,
    terrain_gpu: Option<&TerrainResources>,
    in_use: &mut FxHashSet<WorldTileCoords>,
    coords: WorldTileCoords,
    dem: &crate::terrain::source::DemSource,
) {
    let Some(dem_coords) = dem_tile_coords(coords, dem.minzoom, dem.maxzoom) else {
        return;
    };
    in_use.insert(dem_coords);
    let own_state = tiles.query::<&crate::terrain::DemTileComponent>(dem_coords);
    if matches!(own_state, Some(crate::terrain::DemTileComponent::Loaded(_)))
        && terrain_gpu.is_none_or(|t| t.has_dem_texture(dem_coords))
    {
        return;
    }
    if !matches!(own_state, Some(crate::terrain::DemTileComponent::Missing)) {
        if let Some(culling_ancestor) =
            crate::terrain::request_system::dem_ancestor_coords(dem_coords, dem.minzoom)
        {
            in_use.insert(culling_ancestor);
        }
    }
    let mut current = dem_coords;
    while let Some(parent) = current.get_parent() {
        if u8::from(parent.z) < dem.minzoom {
            break;
        }
        match tiles.query::<&crate::terrain::DemTileComponent>(parent) {
            Some(crate::terrain::DemTileComponent::Loaded(_)) => {
                in_use.insert(parent);
                if terrain_gpu.is_none_or(|t| t.has_dem_texture(parent)) {
                    break;
                }
            }
            Some(crate::terrain::DemTileComponent::Pending) => {
                in_use.insert(parent);
            }
            None if matches!(
                tiles.query::<&crate::terrain::DemTileComponent>(current),
                Some(crate::terrain::DemTileComponent::Missing)
            ) =>
            {
                in_use.insert(parent);
                break;
            }
            _ => {}
        }
        current = parent;
    }
}

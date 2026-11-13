//! Retains only the raster and DEM fallback ancestors actually needed by unsettled tiles.

use rustc_hash::FxHashSet;

use crate::{
    coords::WorldTileCoords,
    io::{
        tile_backpressure::is_settled,
        tile_sources::{RasterSourceId, TileKind},
    },
    style::Style,
    tcs::{tiles::Tiles, world::World},
    terrain::{dem_tile_coords, source::dem_source},
};

/// Appends active drape requests, drape prefetch requests, and only the raster/DEM fallback
/// ancestors actually needed by tiles whose own source data has not yet loaded.
pub(crate) fn finish_tiles_in_use(
    world: &World,
    style: &Style,
    mut in_use: FxHashSet<WorldTileCoords>,
) -> FxHashSet<WorldTileCoords> {
    if let Some(requests) = world
        .resources
        .get::<crate::terrain::request_system::DrapeRequests>()
    {
        in_use.extend(requests.0.iter().copied());
    }
    if let Some(prefetch) = world
        .resources
        .get::<crate::terrain::request_system::DrapePrefetchRequests>()
    {
        in_use.extend(prefetch.0.iter().copied());
    }
    let dem = dem_source(style);
    let raster_sources = active_raster_sources(style);
    if dem.is_none() && raster_sources.is_empty() {
        return in_use;
    }
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
            retain_dem_fallback_ancestors(&world.tiles, &mut in_use, coords, dem);
        }
    }
    in_use
}

fn active_raster_sources(style: &Style) -> Vec<(RasterSourceId, u8)> {
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
    in_use: &mut FxHashSet<WorldTileCoords>,
    coords: WorldTileCoords,
    dem: &crate::terrain::source::DemSource,
) {
    let Some(dem_coords) = dem_tile_coords(coords, dem.minzoom, dem.maxzoom) else {
        return;
    };
    in_use.insert(dem_coords);
    let own_state = tiles.query::<&crate::terrain::DemTileComponent>(dem_coords);
    if matches!(own_state, Some(crate::terrain::DemTileComponent::Loaded(_))) {
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
                break;
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

//! Pre-dispatches initial viewport tile fetches concurrently with GPU adapter/device creation.

use rustc_hash::FxHashSet;

use crate::{
    coords::{OverscaledTileId, WorldTileCoords},
    environment::OffscreenKernel,
    io::{
        apc::{AsyncProcedureCall, Input, ProcedureTag},
        frame_style::FrameStyle,
        source_client::{HttpClient, SourceClient},
        tile_json::resolve_tile_json_sources,
        tile_retry::{self, RequestAttempts, RequestKind},
        tile_sources::{outside_source_bounds, source_max_zoom, source_min_zoom, TileKind},
    },
    render::{
        projection::{terrain_region, view_region_for_projection},
        tile_view_pattern::DEFAULT_TILE_SIZE,
        view_state::{ViewState, ViewStatePadding},
    },
    style::Style,
    tcs::world::World,
    terrain::{
        request_system::{dem_ancestor_coords, dem_tile_coords, TerrainWantedDems},
        source::dem_source,
        DemTileComponent,
    },
    vector::{depends_on_overscaling, VectorLayerBucketComponent},
};

const MAX_PREWARM_VECTOR_TILES: usize = 16;
const MAX_PREWARM_DEM_TILES: usize = 12;
#[cfg(feature = "raster")]
const MAX_PREWARM_RASTER_TILES: usize = 16;

struct PrewarmedVector {
    coords: WorldTileCoords,
    attempt: u64,
    overscaled_zoom: u8,
}

/// Requests dispatched before plugins registered their archetypes on [`World`], committed once
/// `plugin.build(...)` completes so Frame 1 ingests arrived payloads without double-requesting.
#[derive(Default)]
pub(super) struct PrewarmedRequests {
    pixel_ratio: f32,
    requested_vector_layers: Vec<String>,
    vectors: Vec<PrewarmedVector>,
    wanted_vectors: FxHashSet<WorldTileCoords>,
    dems: Vec<(WorldTileCoords, u64)>,
    wanted_dems: std::collections::HashSet<WorldTileCoords>,
    #[cfg(feature = "raster")]
    rasters: Vec<(WorldTileCoords, u64)>,
    #[cfg(feature = "raster")]
    wanted_rasters: FxHashSet<WorldTileCoords>,
}

/// Resolves TileJSON metadata, prefetches symbol assets, warms worker styles, and pre-dispatches
/// initial viewport tile fetches concurrently with renderer creation.
pub(super) async fn prepare_io_and_prewarm<
    K: OffscreenKernel,
    H: HttpClient,
    A: AsyncProcedureCall<K>,
>(
    style: &mut Style,
    view_state: &ViewState,
    scale_factor: f64,
    request_attempts: &RequestAttempts,
    source_client: &SourceClient<H>,
    apc: &A,
) -> PrewarmedRequests {
    resolve_tile_json_sources(style, source_client).await;
    crate::sdf::assets::spawn_style_symbol_prefetch(source_client.clone(), style);
    apc.warm_style(style);
    prewarm_initial_tiles(style, view_state, scale_factor, request_attempts, apc)
}

fn prewarm_initial_tiles<K: OffscreenKernel, A: AsyncProcedureCall<K>>(
    style: &Style,
    view_state: &ViewState,
    scale_factor: f64,
    request_attempts: &RequestAttempts,
    apc: &A,
) -> PrewarmedRequests {
    let mut out = PrewarmedRequests {
        pixel_ratio: (scale_factor as f32).clamp(0.25, 8.0),
        ..Default::default()
    };
    if !apc.supports_prewarm() {
        return out;
    }

    let empty_world = World::default();
    let mut frame_style = FrameStyle::new(style);
    let magnifies = depends_on_overscaling(style);
    let min_zoom = source_min_zoom(style, TileKind::Vector);
    let max_zoom = source_max_zoom(style, TileKind::Vector);
    let worker_style = frame_style.filtered_worker_snapshot(false, false);
    let has_vector = !worker_style.cached_vector_layer_ids().is_empty()
        || worker_style
            .layers
            .iter()
            .any(|l| l.type_ == "symbol" && crate::style::mutation::from_vector_tiles(l));

    if has_vector && style.terrain.is_none() {
        let visible_level = view_state.zoom().zoom_level(DEFAULT_TILE_SIZE);
        let level = u8::from(visible_level);
        if let Ok(Some(region)) = view_region_for_projection(
            style,
            view_state,
            &empty_world,
            visible_level,
            ViewStatePadding::Tight,
        ) {
            out.requested_vector_layers = style.cached_vector_layer_ids().to_vec();
            for raw in region.iter() {
                if out.vectors.len() >= MAX_PREWARM_VECTOR_TILES {
                    break;
                }
                if min_zoom.is_some_and(|min| u8::from(raw.z) < min) {
                    continue;
                }
                let Some(target) = OverscaledTileId::for_source(raw, level, max_zoom, magnifies)
                else {
                    continue;
                };
                let coords = target.canonical;
                if outside_source_bounds(style, TileKind::Vector, coords)
                    || !out.wanted_vectors.insert(coords)
                {
                    continue;
                }
                let overscaled_zoom =
                    if magnifies && max_zoom.is_some_and(|m| u8::from(coords.z) >= m) {
                        target.overscaled_z
                    } else {
                        0
                    };
                let attempt = request_attempts.next();
                if apc
                    .prewarm_call(
                        ProcedureTag::FetchVector,
                        Input {
                            coords,
                            style: worker_style.clone(),
                            attempt,
                            overscaled_zoom,
                            pixel_ratio: out.pixel_ratio,
                            only_sources: None,
                            refetch: false,
                        },
                    )
                    .is_ok()
                {
                    out.vectors.push(PrewarmedVector {
                        coords,
                        attempt,
                        overscaled_zoom,
                    });
                }
            }
        }
    }

    if let Some(dem) = dem_source(style) {
        if let Ok(Some(region)) = terrain_region(
            style,
            view_state,
            &empty_world,
            dem.tile_size,
            ViewStatePadding::Tight,
        ) {
            let mut ordered_dems = Vec::new();
            let mut seen_dems = FxHashSet::default();
            for tile in region.iter() {
                if let Some(dem_coords) = dem_tile_coords(tile, dem.minzoom, dem.maxzoom) {
                    if let Some(ancestor) = dem_ancestor_coords(dem_coords, dem.minzoom) {
                        if seen_dems.insert(ancestor) {
                            ordered_dems.push(ancestor);
                        }
                    }
                    if seen_dems.insert(dem_coords) {
                        ordered_dems.push(dem_coords);
                    }
                }
            }
            let dem_style = frame_style.worker_snapshot();
            for coords in ordered_dems.into_iter().take(MAX_PREWARM_DEM_TILES) {
                if coords.build_quad_key().is_none() || !out.wanted_dems.insert(coords) {
                    continue;
                }
                let attempt = request_attempts.next();
                if apc
                    .prewarm_call(
                        ProcedureTag::FetchDem,
                        Input::new(coords, dem_style.clone(), attempt),
                    )
                    .is_ok()
                {
                    out.dems.push((coords, attempt));
                }
            }
        }
    }

    #[cfg(feature = "raster")]
    if let Ok(regions) = crate::render::projection::raster_source_regions(
        style,
        view_state,
        &empty_world,
        ViewStatePadding::Tight,
    ) {
        let raster_style = frame_style.worker_snapshot();
        'raster_loop: for (source, tiles) in regions {
            if crate::raster::media_source::is_media_source(style, source.name()) {
                continue;
            }
            let bounds = match source.name().and_then(|name| style.sources.get(name)) {
                Some(crate::style::source::Source::Raster(r)) => r.bounds,
                _ => None,
            };
            for coords in tiles {
                if out.rasters.len() >= MAX_PREWARM_RASTER_TILES {
                    break 'raster_loop;
                }
                if bounds.is_some_and(|b| !crate::io::tile_sources::tile_in_bounds(coords, b))
                    || !out.wanted_rasters.insert(coords)
                {
                    continue;
                }
                let attempt = request_attempts.next();
                if apc
                    .prewarm_call(
                        ProcedureTag::FetchRaster,
                        Input::new(coords, raster_style.clone(), attempt),
                    )
                    .is_ok()
                {
                    out.rasters.push((coords, attempt));
                }
            }
        }
    }

    out
}

impl PrewarmedRequests {
    pub(super) fn commit_into(self, world: &mut World) {
        if !self.vectors.is_empty() {
            for item in self.vectors {
                if world.tiles.spawn_mut(item.coords).is_none() {
                    continue;
                }
                crate::vector::content::begin(
                    world,
                    item.coords,
                    self.requested_vector_layers.clone(),
                );
                if let Some(tile) = world
                    .tiles
                    .query_mut::<&mut VectorLayerBucketComponent>(item.coords)
                {
                    tile.overscaled_zoom = item.overscaled_zoom;
                }
                tile_retry::started(world, item.coords, RequestKind::Vector, item.attempt, None);
                let _ = crate::sdf::provided::restarted(world, item.coords, self.pixel_ratio);
            }
            tile_retry::want(world, RequestKind::Vector, &self.wanted_vectors);
        }

        if !self.dems.is_empty() {
            for (coords, attempt) in self.dems {
                if let Some(mut tile) = world.tiles.spawn_mut(coords) {
                    tile.insert(DemTileComponent::Pending);
                    tile_retry::started(world, coords, RequestKind::Dem, attempt, None);
                }
            }
            tile_retry::want(world, RequestKind::Dem, &self.wanted_dems);
            world.resources.insert(TerrainWantedDems(self.wanted_dems));
        }

        #[cfg(feature = "raster")]
        if !self.rasters.is_empty() {
            for (coords, attempt) in self.rasters {
                if let Some(mut tile) = world.tiles.spawn_mut(coords) {
                    tile.insert(crate::raster::RasterLayersDataComponent::default());
                    tile_retry::started(world, coords, RequestKind::Raster, attempt, None);
                }
            }
            tile_retry::want(world, RequestKind::Raster, &self.wanted_rasters);
        }
    }
}

//! Pre-dispatches initial viewport tile fetches concurrently with GPU adapter/device creation.

use std::sync::Arc;

use rustc_hash::FxHashSet;

use crate::{
    coords::{OverscaledTileId, WorldTileCoords, ZoomLevel},
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
        dem::dem_tile_coords,
        request_system::{dem_ancestor_coords, TerrainWantedDems},
        source::dem_source,
        DemTileComponent,
    },
    vector::{depends_on_overscaling, VectorLayerBucketComponent},
};

const MAX_PREWARM_VECTOR_TILES: usize = 16;
const MAX_PREWARM_TERRAIN_VECTOR_TILES: usize = 10;
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
    requested_vector_layers: Option<Arc<[String]>>,
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
    #[cfg(not(all(target_arch = "wasm32", target_feature = "atomics")))]
    crate::sdf::assets::spawn_style_symbol_prefetch(source_client.clone(), style);
    #[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
    apc.warm_style(style);
    resolve_tile_json_sources(style, source_client).await;
    #[cfg(not(all(target_arch = "wasm32", target_feature = "atomics")))]
    apc.warm_style(style);
    prewarm_initial_tiles(style, view_state, scale_factor, request_attempts, apc)
}

struct PrewarmContext<'a> {
    style: &'a Style,
    view_state: &'a ViewState,
    empty_world: &'a World,
    request_attempts: &'a RequestAttempts,
}

struct VectorDispatchState<'a, K: OffscreenKernel, A: AsyncProcedureCall<K>> {
    ctx: &'a PrewarmContext<'a>,
    apc: &'a A,
    worker_style: &'a std::sync::Arc<Style>,
    max_vector_tiles: usize,
    out: &'a mut PrewarmedRequests,
    _kernel: std::marker::PhantomData<fn() -> K>,
}

fn dispatch_vector_tile<K: OffscreenKernel, A: AsyncProcedureCall<K>>(
    state: &mut VectorDispatchState<'_, K, A>,
    coords: WorldTileCoords,
    overscaled_zoom: u8,
) {
    if state.out.vectors.len() >= state.max_vector_tiles
        || outside_source_bounds(state.ctx.style, TileKind::Vector, coords)
        || !state.out.wanted_vectors.insert(coords)
    {
        return;
    }
    let attempt = state.ctx.request_attempts.next();
    if state
        .apc
        .prewarm_call(
            ProcedureTag::FetchVector,
            Input {
                coords,
                style: state.worker_style.clone(),
                attempt,
                overscaled_zoom,
                pixel_ratio: state.out.pixel_ratio,
                only_sources: None,
                refetch: false,
            },
            crate::vector::fetch_vector_apc::<
                K,
                crate::vector::DefaultVectorTransferables,
                A::Context,
            >,
        )
        .is_ok()
    {
        state.out.vectors.push(PrewarmedVector {
            coords,
            attempt,
            overscaled_zoom,
        });
    }
}

fn prewarm_vector_tiles<K: OffscreenKernel, A: AsyncProcedureCall<K>>(
    ctx: &PrewarmContext<'_>,
    frame_style: &mut FrameStyle,
    apc: &A,
    out: &mut PrewarmedRequests,
) {
    let worker_style = frame_style.filtered_worker_snapshot(false, false);
    let has_vector = !worker_style.cached_vector_layer_ids().is_empty()
        || worker_style
            .layers
            .iter()
            .any(|l| l.type_ == "symbol" && crate::style::mutation::from_vector_tiles(l));
    if !has_vector {
        return;
    }
    let style = ctx.style;
    let magnifies = depends_on_overscaling(style);
    let min_zoom = source_min_zoom(style, TileKind::Vector);
    let max_zoom = source_max_zoom(style, TileKind::Vector);
    let visible_level = ctx.view_state.zoom().zoom_level(DEFAULT_TILE_SIZE);
    let level = u8::from(visible_level);
    let max_vector_tiles = if style.terrain.is_some() {
        MAX_PREWARM_TERRAIN_VECTOR_TILES
    } else {
        MAX_PREWARM_VECTOR_TILES
    };
    out.requested_vector_layers = Some(style.cached_vector_layer_ids());
    let mut state = VectorDispatchState {
        ctx,
        apc,
        worker_style: &worker_style,
        max_vector_tiles,
        out,
        _kernel: std::marker::PhantomData,
    };
    if style.terrain.is_some() && min_zoom.is_none_or(|min| min == 0) {
        let root = WorldTileCoords {
            x: 0,
            y: 0,
            z: ZoomLevel::new(0),
        };
        dispatch_vector_tile(&mut state, root, 0);
    }
    let Ok(Some(region)) = view_region_for_projection(
        style,
        ctx.view_state,
        ctx.empty_world,
        visible_level,
        ViewStatePadding::Tight,
    ) else {
        return;
    };
    for raw in region.iter() {
        if min_zoom.is_some_and(|min| u8::from(raw.z) < min) {
            continue;
        }
        let Some(target) = OverscaledTileId::for_source(raw, level, max_zoom, magnifies) else {
            continue;
        };
        let coords = target.canonical;
        let overscaled_zoom = if magnifies && max_zoom.is_some_and(|m| u8::from(coords.z) >= m) {
            target.overscaled_z
        } else {
            0
        };
        dispatch_vector_tile(&mut state, coords, overscaled_zoom);
    }
}

fn prewarm_dem_tiles<K: OffscreenKernel, A: AsyncProcedureCall<K>>(
    ctx: &PrewarmContext<'_>,
    frame_style: &mut FrameStyle,
    apc: &A,
    out: &mut PrewarmedRequests,
) {
    let Some(dem) = dem_source(ctx.style) else {
        return;
    };
    let Ok(Some(region)) = terrain_region(
        ctx.style,
        ctx.view_state,
        ctx.empty_world,
        dem.tile_size,
        ViewStatePadding::Tight,
    ) else {
        return;
    };
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
        let attempt = ctx.request_attempts.next();
        if apc
            .prewarm_call(
                ProcedureTag::FetchDem,
                Input::new(coords, dem_style.clone(), attempt),
                crate::terrain::fetch_dem_apc::<
                    K,
                    crate::terrain::DefaultDemTransferables,
                    A::Context,
                >,
            )
            .is_ok()
        {
            out.dems.push((coords, attempt));
        }
    }
}

#[cfg(feature = "raster")]
fn prewarm_raster_tiles<K: OffscreenKernel, A: AsyncProcedureCall<K>>(
    ctx: &PrewarmContext<'_>,
    frame_style: &mut FrameStyle,
    apc: &A,
    out: &mut PrewarmedRequests,
) {
    let Ok(regions) = crate::render::eye_covering::raster_source_regions(
        ctx.style,
        ctx.view_state,
        ctx.empty_world,
        ViewStatePadding::Tight,
    ) else {
        return;
    };
    let raster_style = frame_style.worker_snapshot();
    'raster_loop: for (source, tiles) in regions {
        if crate::raster::media_source::is_media_source(ctx.style, source.name()) {
            continue;
        }
        let bounds = match source.name().and_then(|name| ctx.style.sources.get(name)) {
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
            let attempt = ctx.request_attempts.next();
            if apc
                .prewarm_call(
                    ProcedureTag::FetchRaster,
                    Input::new(coords, raster_style.clone(), attempt),
                    crate::raster::fetch_raster_apc::<
                        K,
                        crate::raster::DefaultRasterTransferables,
                        A::Context,
                    >,
                )
                .is_ok()
            {
                out.rasters.push((coords, attempt));
            }
        }
    }
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
    let ctx = PrewarmContext {
        style,
        view_state,
        empty_world: &empty_world,
        request_attempts,
    };
    prewarm_vector_tiles(&ctx, &mut frame_style, apc, &mut out);
    prewarm_dem_tiles(&ctx, &mut frame_style, apc, &mut out);
    #[cfg(feature = "raster")]
    prewarm_raster_tiles(&ctx, &mut frame_style, apc, &mut out);
    apc.flush_calls();
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
                    self.requested_vector_layers
                        .clone()
                        .unwrap_or_else(|| Arc::from([])),
                );
                if let Some(tile) = world
                    .tiles
                    .query_mut::<&mut VectorLayerBucketComponent>(item.coords)
                {
                    tile.overscaled_zoom = item.overscaled_zoom;
                }
                tile_retry::started(world, item.coords, RequestKind::Vector, item.attempt, None);
                let _restarted =
                    crate::sdf::provided::restarted(world, item.coords, self.pixel_ratio);
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
            world
                .resources
                .insert(TerrainWantedDems(self.wanted_dems.into_iter().collect()));
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

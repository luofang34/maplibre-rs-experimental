//! Requests the DEM tiles covering the tiles in view.

use std::{borrow::Cow, collections::HashSet, marker::PhantomData, rc::Rc};

use crate::{
    context::MapContext,
    coords::WorldTileCoords,
    environment::{Environment, OffscreenKernel},
    io::{
        apc::{
            AsyncProcedureCall, AsyncProcedureFuture, AttemptContext, Context, Input,
            ProcedureError, ProcedureTag,
        },
        frame_style::FrameStyle,
        tile_backpressure::dem_request_budget,
        tile_retry::{self, RequestDisposition, RequestKind, TileRequestOutcome},
        tile_sources::{missing_tile_fallback, RASTER_LAYER_TYPES},
    },
    kernel::Kernel,
    render::view_state::ViewStatePadding,
    tcs::{
        system::{System, SystemError, SystemResult},
        tiles::Tiles,
    },
    terrain::{
        source::dem_source,
        transferables::{DemTransferables, LayerDem, LayerDemMissing},
        DemTileComponent,
    },
};

/// The terrain coverage queued for drawing; requests must include these coarsened tiles.
#[derive(Default)]
pub(crate) struct DrapeRequests(pub(crate) Vec<WorldTileCoords>);

/// Tiles prepared just outside the displayed covering, after visible requests.
#[derive(Default)]
pub(crate) struct DrapePrefetchRequests(pub(crate) Vec<WorldTileCoords>);

/// Exact DEM tile coordinates requested by 3D terrain (`dem_tile_coords` + coarse ancestors),
/// excluding higher-zoom hillshade/color-relief tiles that share `GpuDemPool`.
#[derive(Default)]
pub(crate) struct TerrainWantedDems(pub(crate) HashSet<WorldTileCoords>);

/// Zoom levels between a draped tile and the DEM tile it samples, as in GL JS `deltaZoom`.
const DELTA_ZOOM: u8 = 1;
/// Zoom of the coarse ancestor loaded alongside every DEM tile so tile culling knows the
/// elevation range of an area before its own tiles arrive, as in GL JS `_addTerrainIdealTiles`.
const ANCESTOR_ZOOM: u8 = 5;

/// Returns the DEM tile a view tile samples: one zoom level up, clamped to the source range.
pub fn dem_tile_coords(
    coords: WorldTileCoords,
    minzoom: u8,
    maxzoom: u8,
) -> Option<WorldTileCoords> {
    let zoom = u8::from(coords.z).saturating_sub(DELTA_ZOOM).min(maxzoom);
    if zoom < minzoom {
        return None;
    }
    let mut current = coords;
    while u8::from(current.z) > zoom {
        current = current.get_parent()?;
    }
    Some(current)
}

/// Returns the coarse ancestor requested together with a DEM tile, if it differs from it.
pub fn dem_ancestor_coords(coords: WorldTileCoords, minzoom: u8) -> Option<WorldTileCoords> {
    let target = u8::from(coords.z).min(ANCESTOR_ZOOM).max(minzoom);
    if target >= u8::from(coords.z) {
        return None;
    }
    let mut current = coords;
    while u8::from(current.z) > target {
        current = current.get_parent()?;
    }
    Some(current)
}

/// The ancestor to request for a DEM tile the source does not have.
pub fn missing_dem_fallback(
    tiles: &Tiles,
    coords: WorldTileCoords,
    minzoom: u8,
) -> Option<WorldTileCoords> {
    missing_tile_fallback(coords, minzoom, |coords| {
        matches!(
            tiles.query::<&DemTileComponent>(coords),
            Some(DemTileComponent::Missing)
        )
    })
}

pub struct RequestSystem<E: Environment, T: DemTransferables> {
    kernel: Rc<Kernel<E>>,
    phantom_t: PhantomData<T>,
}

impl<E: Environment, T: DemTransferables> RequestSystem<E, T> {
    pub fn new(kernel: &Rc<Kernel<E>>) -> Self {
        Self {
            kernel: kernel.clone(),
            phantom_t: PhantomData,
        }
    }
}

impl<E: Environment, T: DemTransferables> System for RequestSystem<E, T> {
    fn name(&self) -> Cow<'static, str> {
        "dem_request".into()
    }

    fn run(
        &mut self,
        MapContext {
            style,
            view_state,
            world,
            ..
        }: &mut MapContext,
    ) -> SystemResult {
        tile_retry::stop_cancelled(world, self.kernel.apc());
        crate::io::tile_backpressure::set_dem_deferred(world, false);
        let Some(dem) = dem_source(style) else {
            return Ok(());
        };
        let multi = world
            .resources
            .get::<crate::render::eye_covering::MultiViewExtractCovering>()
            .filter(|m| m.active)
            .cloned();
        let terrain_tiles = if let Some(multi) = &multi {
            multi.terrain_tiles.clone()
        } else {
            let view_region = crate::render::projection::terrain_region(
                style,
                view_state,
                world,
                dem.tile_size,
                ViewStatePadding::Tight,
            )?;
            let key = crate::terrain::coverage::PreparedTerrainKey::capture(
                view_state,
                world,
                dem.tile_size,
            );
            world
                .resources
                .insert(crate::terrain::coverage::PreparedTerrainRegion {
                    key,
                    region: view_region.clone(),
                });
            let Some(view_region) = view_region else {
                return Ok(());
            };
            let order_key = crate::terrain::queue_system::draw_order_key(
                style,
                view_state,
                f64::from(dem.tile_size),
            );
            let mut ordered: Vec<WorldTileCoords> = view_region.iter().collect();
            ordered.sort_by(|left, right| order_key(left).total_cmp(&order_key(right)));
            ordered
        };

        let mut requested = HashSet::new();
        let has_raster = style
            .layers
            .iter()
            .any(|layer| RASTER_LAYER_TYPES.contains(&layer.type_.as_str()) && !layer.is_hidden());
        let mut budget = dem_request_budget(world, has_raster);
        let drapes = world
            .resources
            .get::<DrapeRequests>()
            .map(|requests| requests.0.clone())
            .unwrap_or_default();
        let prefetch = world
            .resources
            .get::<DrapePrefetchRequests>()
            .map(|requests| requests.0.clone())
            .unwrap_or_default();
        let mut wanted = request_order(
            terrain_tiles.into_iter().chain(drapes),
            &world.tiles,
            dem.minzoom,
            dem.maxzoom,
        );
        wanted.extend(request_order(
            prefetch.into_iter(),
            &world.tiles,
            dem.minzoom,
            dem.maxzoom,
        ));
        let mut deferred = false;
        let mut frame_style = FrameStyle::new(style);
        for coords in wanted {
            if coords.build_quad_key().is_none() || !requested.insert(coords) {
                continue;
            }
            if tile_retry::backing_off(world, coords, RequestKind::Dem)
                || world.tiles.query::<&DemTileComponent>(coords).is_some()
                    && !tile_retry::due(world, coords, RequestKind::Dem)
            {
                continue;
            }
            // The rest wait for a later frame, once tiles in flight have landed.
            if budget == 0 {
                deferred = true;
                continue;
            }
            budget -= 1;
            // One rejected tile must not cost the frame its other tiles.
            if let Err(error) = self.request(coords, &mut frame_style, world) {
                tracing::warn!(%error, "tile request rejected; it is requested again after a backoff");
                tile_retry::rejected(world, coords, RequestKind::Dem, None);
            }
        }
        crate::io::tile_backpressure::set_dem_deferred(world, deferred);
        if let Some(multi) = &multi {
            requested.extend(multi.skipped_wanted_dem.iter().copied());
        }
        tile_retry::want(world, RequestKind::Dem, &requested);
        world.resources.insert(TerrainWantedDems(requested));

        Ok(())
    }
}

impl<E: Environment, T: DemTransferables> RequestSystem<E, T> {
    fn request(
        &self,
        coords: WorldTileCoords,
        style: &mut FrameStyle,
        world: &mut crate::tcs::world::World,
    ) -> SystemResult {
        let exists = world.tiles.query::<&DemTileComponent>(coords).is_some();
        let attempt = tile_retry::next_attempt(world);
        let Some(mut tile) = world.tiles.spawn_mut(coords) else {
            return Err(SystemError::InvalidTile { coords });
        };
        self.kernel
            .apc()
            .call_tagged(
                ProcedureTag::FetchDem,
                Input::new(coords, style.worker_snapshot(), attempt),
                fetch_dem_apc::<E::OffscreenKernelEnvironment, T, _>,
            )
            .map_err(|source| SystemError::TileRequest {
                kind: "DEM",
                coords,
                source,
            })?;
        if !exists {
            tile.insert(DemTileComponent::Pending);
        }
        tile_retry::started(world, coords, RequestKind::Dem, attempt, None);
        tracing::debug!(%coords, "DEM tile request accepted");
        Ok(())
    }
}

/// Fetches and decodes one DEM tile on a worker.
pub fn fetch_dem_apc<K: OffscreenKernel, T: DemTransferables, C: Context + Clone + Send>(
    input: Input,
    context: C,
    kernel: K,
) -> AsyncProcedureFuture {
    Box::pin(async move {
        let Input {
            coords,
            style,
            attempt,
            ..
        } = input;
        let context = AttemptContext::new(context, attempt);
        let Some(dem) = dem_source(&style) else {
            return Ok(());
        };
        let client = kernel.source_client();
        let mut retry = false;
        use crate::{projection::body::Body, sdf::assets::tiles::CachedTileFailure};
        let circumference = Body::EARTH.circumference_meters();
        let loaded = match client
            .assets()
            .load_dem_tile(&client, &dem.source, &coords, dem.unpack, circumference)
            .await
        {
            Ok(entry) => Some(entry),
            Err(CachedTileFailure::NotFound) => {
                tracing::debug!(%coords, source = %dem.name, "no DEM tile at the source; flat");
                None
            }
            Err(CachedTileFailure::Decode(error)) => {
                tracing::warn!(%coords, source = %dem.name, %error, "DEM tile is not an image");
                None
            }
            Err(CachedTileFailure::Transport { retryable, message }) => {
                retry = retryable;
                if retryable {
                    tracing::warn!(
                        %coords,
                        source = %dem.name,
                        error = %message,
                        "DEM tile fetch failed"
                    );
                } else {
                    tracing::error!(
                        %coords,
                        source = %dem.name,
                        error = %message,
                        "DEM tile fetch failed"
                    );
                }
                None
            }
        };
        match loaded {
            Some(entry) => context
                .send_back(T::LayerDem::build_from_cached_dem(
                    coords,
                    std::sync::Arc::clone(&entry.tile),
                    entry.circumference_meters,
                    std::sync::Arc::clone(&entry.image),
                ))
                .map_err(ProcedureError::Send),
            None => context
                .send_back(T::LayerDemMissing::build_from(coords))
                .map_err(ProcedureError::Send),
        }?;
        context
            .send_back(TileRequestOutcome {
                coords,
                kind: RequestKind::Dem,
                attempt: Some(attempt),
                disposition: if retry {
                    RequestDisposition::Retry
                } else {
                    RequestDisposition::Complete
                },
            })
            .map_err(ProcedureError::Send)
    })
}

#[cfg(test)]
mod tests;

// Foreground DEMs must not queue behind coarse textures: their resolutions are independent.
fn request_order(
    tiles: impl Iterator<Item = WorldTileCoords>,
    loaded: &Tiles,
    minzoom: u8,
    maxzoom: u8,
) -> Vec<WorldTileCoords> {
    let mut seen = HashSet::new();
    let fine: Vec<_> = tiles
        .filter_map(|coords| dem_tile_coords(coords, minzoom, maxzoom))
        .filter(|coords| seen.insert(*coords))
        .collect();
    let mut result = Vec::new();
    for coords in &fine {
        let already_loaded = matches!(
            loaded.query::<&DemTileComponent>(*coords),
            Some(DemTileComponent::Loaded(_))
        );
        if !already_loaded {
            if let Some(parent) = dem_ancestor_coords(*coords, minzoom) {
                if seen.insert(parent) {
                    result.push(parent);
                }
            }
        }
    }
    // Up to four coarse `z5` tiles bracket viewports straddling continental tile boundaries
    // (e.g. Innsbruck near `11.25° E`) before fine DEM tiles stream in.
    let remaining = result.split_off(result.len().min(4));
    result.extend(fine.iter().copied());
    result.extend(remaining);
    result.extend(
        fine.into_iter()
            .filter_map(|coords| missing_dem_fallback(loaded, coords, minzoom)),
    );
    result
}

//! Requests the DEM tiles covering the tiles in view.

mod memo;

use std::{borrow::Cow, marker::PhantomData, rc::Rc};

use rustc_hash::FxHashSet;

use crate::{
    context::MapContext,
    coords::WorldTileCoords,
    environment::{Environment, OffscreenKernel},
    io::{
        apc::{
            AsyncProcedureCall, AsyncProcedureFuture, AttemptContext, Context, Input, IntoMessage,
            ProcedureError, ProcedureTag,
        },
        frame_style::FrameStyle,
        source_client::limiter::{SharedTileClaim, SharedTileSlot},
        tile_backpressure::dem_request_budget,
        tile_retry::{self, RequestDisposition, RequestKind, TileRequestOutcome},
        tile_sources::{missing_tile_fallback, RASTER_LAYER_TYPES},
    },
    kernel::Kernel,
    render::view_state::ViewStatePadding,
    schedule::system::{System, SystemError, SystemResult},
    tcs::tiles::Tiles,
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
pub(crate) struct TerrainWantedDems(pub(crate) FxHashSet<WorldTileCoords>);

/// Monotonic epoch bumped whenever [`TerrainWantedDems`] membership changes.
#[derive(Default)]
pub(crate) struct TerrainWantedDemsEpoch(pub u64);

pub(crate) fn wanted_dems_epoch(world: &crate::tcs::world::World) -> u64 {
    world
        .resources
        .get::<TerrainWantedDemsEpoch>()
        .map_or(0, |epoch| epoch.0)
}

/// Zoom of the coarse ancestor loaded alongside every DEM tile so tile culling knows the
/// elevation range of an area before its own tiles arrive, as in GL JS `_addTerrainIdealTiles`.
const ANCESTOR_ZOOM: u8 = 5;

use crate::terrain::dem::dem_tile_coords;

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
    memo: memo::DemRequestMemo,
    phantom_t: PhantomData<T>,
}

impl<E: Environment, T: DemTransferables> RequestSystem<E, T> {
    pub fn new(kernel: &Rc<Kernel<E>>) -> Self {
        Self {
            kernel: kernel.clone(),
            memo: memo::DemRequestMemo::default(),
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
        let Some(dem) = dem_source(style) else {
            crate::io::tile_backpressure::set_dem_deferred(world, false);
            self.memo.invalidate();
            return Ok(());
        };
        let multi = world
            .resources
            .get::<crate::render::eye_covering::MultiViewExtractCovering>()
            .filter(|m| m.active)
            .cloned();
        let memo_key = memo::DemRequestMemo::build_key(
            world,
            style,
            view_state,
            dem.tile_size,
            multi.as_ref(),
        );
        if memo_key
            .as_ref()
            .is_some_and(|key| self.memo.try_reuse(world, key))
        {
            return Ok(());
        }
        crate::io::tile_backpressure::set_dem_deferred(world, false);
        let (terrain_tiles, fresh_region) = match &multi {
            Some(m) => (m.terrain_tiles.clone(), None),
            None => {
                let Some(tiles_and_region) =
                    resolve_single_view_tiles(style, view_state, world, &dem)?
                else {
                    return Ok(());
                };
                (tiles_and_region.0, Some(tiles_and_region.1))
            }
        };
        self.dispatch_wanted(style, world, &dem, &terrain_tiles, multi.as_ref());
        if let Some(memo_key) = memo_key {
            self.memo.record(memo_key, world.tiles.len(), fresh_region);
        }
        Ok(())
    }
}

type SingleViewTiles = (
    Vec<WorldTileCoords>,
    (
        crate::terrain::coverage::PreparedTerrainKey,
        crate::coords::ViewRegion,
    ),
);

fn resolve_single_view_tiles(
    style: &crate::style::Style,
    view_state: &crate::render::view_state::ViewState,
    world: &mut crate::tcs::world::World,
    dem: &crate::terrain::source::DemSource,
) -> Result<Option<SingleViewTiles>, SystemError> {
    let view_region = crate::render::projection::terrain_region(
        style,
        view_state,
        world,
        dem.tile_size,
        ViewStatePadding::Tight,
    )?;
    let key = crate::terrain::coverage::PreparedTerrainKey::capture_for_style(
        style,
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
        return Ok(None);
    };
    let resolved = world
        .resources
        .get::<crate::render::resolved_view::ResolvedFrameView>()
        .filter(|resolved| resolved.matches(style, view_state));
    let order_key = crate::terrain::coverage::draw_order_key_with_resolved(
        resolved,
        style,
        view_state,
        f64::from(dem.tile_size),
    );
    let mut keyed: Vec<(f64, WorldTileCoords)> =
        view_region.iter().map(|c| (order_key(&c), c)).collect();
    keyed.sort_unstable_by(|left, right| left.0.total_cmp(&right.0));
    let ordered = keyed.into_iter().map(|(_, c)| c).collect();
    Ok(Some((ordered, (key, view_region))))
}

impl<E: Environment, T: DemTransferables> RequestSystem<E, T> {
    fn dispatch_wanted(
        &self,
        style: &crate::style::Style,
        world: &mut crate::tcs::world::World,
        dem: &crate::terrain::source::DemSource,
        terrain_tiles: &[WorldTileCoords],
        multi: Option<&crate::render::eye_covering::MultiViewExtractCovering>,
    ) {
        let mut requested = FxHashSet::default();
        let has_raster = style.cached_has_visible_raster_type(&RASTER_LAYER_TYPES);
        let mut budget = dem_request_budget(world, has_raster);
        let mut wanted = {
            let drapes = world
                .resources
                .get::<DrapeRequests>()
                .map(|r| r.0.as_slice())
                .unwrap_or(&[]);
            let prefetch = world
                .resources
                .get::<DrapePrefetchRequests>()
                .map(|r| r.0.as_slice())
                .unwrap_or(&[]);
            let mut list = request_order(
                terrain_tiles.iter().copied().chain(drapes.iter().copied()),
                &world.tiles,
                dem.minzoom,
                dem.maxzoom,
            );
            list.extend(request_order(
                prefetch.iter().copied(),
                &world.tiles,
                dem.minzoom,
                dem.maxzoom,
            ));
            list
        };
        let mut deferred = false;
        let mut frame_style = FrameStyle::new(style);
        for coords in wanted.drain(..) {
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
        if let Some(multi) = multi {
            requested.extend(multi.skipped_wanted_dem.iter().copied());
        }
        tile_retry::want(world, RequestKind::Dem, &requested);
        memo::store_wanted_dems(world, requested);
    }
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
        let (context, shared_slot, retry) =
            load_and_send_dem::<K, T, C>(kernel, context, dem, coords).await?;
        let outcome = IntoMessage::into(TileRequestOutcome {
            coords,
            kind: RequestKind::Dem,
            attempt: Some(attempt),
            disposition: if retry {
                RequestDisposition::Retry
            } else {
                RequestDisposition::Complete
            },
        });
        context
            .send_back(outcome.with_shared_slot(shared_slot))
            .map_err(ProcedureError::Send)
    })
}

async fn load_and_send_dem<K: OffscreenKernel, T: DemTransferables, C: Context>(
    kernel: K,
    context: AttemptContext<C>,
    dem: crate::terrain::source::DemSource,
    coords: WorldTileCoords,
) -> Result<(AttemptContext<C>, SharedTileSlot, bool), ProcedureError> {
    use crate::{io::source_client::tile_cache::CachedTileFailure, projection::body::Body};
    let client = kernel.source_client();
    let circumference = Body::EARTH.circumference_meters();
    let claim = SharedTileClaim::default();
    let mut retry = false;
    let (loaded, shared_slot) = match crate::terrain::dem::cache::load_dem_tile(
        client.assets(),
        &client,
        &dem.source,
        &coords,
        dem.unpack,
        circumference,
        &claim,
    )
    .await
    {
        Ok((entry, slot)) => (Some(entry), slot),
        Err(CachedTileFailure::NotFound) => {
            tracing::debug!(%coords, source = %dem.name, "no DEM tile at the source; flat");
            (None, SharedTileSlot::default())
        }
        Err(CachedTileFailure::Decode(error)) => {
            tracing::warn!(%coords, source = %dem.name, %error, "DEM tile is not an image");
            (None, SharedTileSlot::default())
        }
        Err(CachedTileFailure::Transport { retryable, message }) => {
            retry = retryable;
            if retryable {
                tracing::warn!(%coords, source = %dem.name, error = %message, "DEM tile fetch failed");
            } else {
                tracing::error!(%coords, source = %dem.name, error = %message, "DEM tile fetch failed");
            }
            (None, SharedTileSlot::default())
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
    Ok((context, shared_slot, retry))
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
    let mut seen = FxHashSet::default();
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

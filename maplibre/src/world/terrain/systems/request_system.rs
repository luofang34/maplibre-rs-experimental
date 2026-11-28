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
        dem::dem_tile_coords,
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
    scratch: memo::DemRequestScratch,
    phantom_t: PhantomData<T>,
}

impl<E: Environment, T: DemTransferables> RequestSystem<E, T> {
    pub fn new(kernel: &Rc<Kernel<E>>) -> Self {
        Self {
            kernel: kernel.clone(),
            memo: memo::DemRequestMemo::default(),
            scratch: memo::DemRequestScratch::default(),
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
        let fresh_region = if multi.is_some() {
            None
        } else if let Some(region) =
            resolve_single_view_tiles(style, view_state, world, &dem, &mut self.scratch)?
        {
            Some(region)
        } else {
            return Ok(());
        };
        dispatch_wanted::<E, T>(
            &self.kernel,
            &mut self.scratch,
            style,
            world,
            &dem,
            multi.as_ref(),
        );
        if let Some(memo_key) = memo_key {
            self.memo.record(memo_key, world.tiles.len(), fresh_region);
        }
        Ok(())
    }
}

fn resolve_single_view_tiles(
    style: &crate::style::Style,
    view_state: &crate::render::view_state::ViewState,
    world: &mut crate::tcs::world::World,
    dem: &crate::terrain::source::DemSource,
    scratch: &mut memo::DemRequestScratch,
) -> Result<
    Option<(
        crate::terrain::coverage::PreparedTerrainKey,
        crate::coords::ViewRegion,
    )>,
    SystemError,
> {
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
    if let Some(slot) = world
        .resources
        .get_mut::<crate::terrain::coverage::PreparedTerrainRegion>()
    {
        slot.key = key;
        slot.region = view_region.clone();
    } else {
        world
            .resources
            .insert(crate::terrain::coverage::PreparedTerrainRegion {
                key,
                region: view_region.clone(),
            });
    }
    let Some(view_region) = view_region else {
        scratch.single_view_tiles.clear();
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
    scratch.keyed.clear();
    scratch
        .keyed
        .extend(view_region.iter().map(|c| (order_key(&c), c)));
    scratch
        .keyed
        .sort_unstable_by(|left, right| left.0.total_cmp(&right.0));
    scratch.single_view_tiles.clear();
    scratch
        .single_view_tiles
        .extend(scratch.keyed.iter().map(|(_, c)| *c));
    Ok(Some((key, view_region)))
}

fn dispatch_wanted<E: Environment, T: DemTransferables>(
    kernel: &Rc<Kernel<E>>,
    scratch: &mut memo::DemRequestScratch,
    style: &crate::style::Style,
    world: &mut crate::tcs::world::World,
    dem: &crate::terrain::source::DemSource,
    multi: Option<&crate::render::eye_covering::MultiViewExtractCovering>,
) {
    scratch.requested.clear();
    let has_raster = style.cached_has_visible_raster_type(&RASTER_LAYER_TYPES);
    let mut budget = dem_request_budget(world, has_raster);
    fill_wanted(world, scratch, dem, multi);
    let mut deferred = false;
    let mut frame_style = FrameStyle::new(style);
    for coords in scratch.wanted.drain(..) {
        if coords.build_quad_key().is_none() || !scratch.requested.insert(coords) {
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
        if let Err(error) = request_dem_tile::<E, T>(kernel, coords, &mut frame_style, world) {
            tracing::warn!(%error, "tile request rejected; it is requested again after a backoff");
            tile_retry::rejected(world, coords, RequestKind::Dem, None);
        }
    }
    crate::io::tile_backpressure::set_dem_deferred(world, deferred);
    if let Some(multi) = multi {
        scratch
            .requested
            .extend(multi.skipped_wanted_dem.iter().copied());
    }
    tile_retry::want(world, RequestKind::Dem, &scratch.requested);
    memo::store_wanted_dems(world, &scratch.requested);
}

fn request_dem_tile<E: Environment, T: DemTransferables>(
    kernel: &Rc<Kernel<E>>,
    coords: WorldTileCoords,
    style: &mut FrameStyle,
    world: &mut crate::tcs::world::World,
) -> SystemResult {
    let exists = world.tiles.query::<&DemTileComponent>(coords).is_some();
    let attempt = tile_retry::next_attempt(world);
    let Some(mut tile) = world.tiles.spawn_mut(coords) else {
        return Err(SystemError::InvalidTile { coords });
    };
    kernel
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
fn fill_wanted(
    world: &crate::tcs::world::World,
    scratch: &mut memo::DemRequestScratch,
    dem: &crate::terrain::source::DemSource,
    multi: Option<&crate::render::eye_covering::MultiViewExtractCovering>,
) {
    scratch.wanted.clear();
    let drapes = world
        .resources
        .get::<DrapeRequests>()
        .map_or(&[][..], |r| r.0.as_slice());
    let prefetch = world
        .resources
        .get::<DrapePrefetchRequests>()
        .map_or(&[][..], |r| r.0.as_slice());
    let terrain_tiles = multi.map_or(scratch.single_view_tiles.as_slice(), |m| {
        m.terrain_tiles.as_slice()
    });
    append_ordered_into(
        terrain_tiles.iter().copied().chain(drapes.iter().copied()),
        &world.tiles,
        (dem.minzoom, dem.maxzoom),
        (
            &mut scratch.seen,
            &mut scratch.fine,
            &mut scratch.remaining,
            &mut scratch.wanted,
        ),
    );
    append_ordered_into(
        prefetch.iter().copied(),
        &world.tiles,
        (dem.minzoom, dem.maxzoom),
        (
            &mut scratch.seen,
            &mut scratch.fine,
            &mut scratch.remaining,
            &mut scratch.wanted,
        ),
    );
}

fn append_ordered_into(
    tiles: impl Iterator<Item = WorldTileCoords>,
    loaded: &Tiles,
    (minzoom, maxzoom): (u8, u8),
    (seen, fine, remaining, out): (
        &mut FxHashSet<WorldTileCoords>,
        &mut Vec<WorldTileCoords>,
        &mut Vec<WorldTileCoords>,
        &mut Vec<WorldTileCoords>,
    ),
) {
    seen.clear();
    fine.clear();
    remaining.clear();
    for coords in tiles {
        if let Some(dem_coords) = dem_tile_coords(coords, minzoom, maxzoom)
            && seen.insert(dem_coords)
        {
            fine.push(dem_coords);
        }
    }
    let ancestor_start = out.len();
    for &coords in fine.iter() {
        let already_loaded = matches!(
            loaded.query::<&DemTileComponent>(coords),
            Some(DemTileComponent::Loaded(_))
        );
        if !already_loaded
            && let Some(parent) = dem_ancestor_coords(coords, minzoom)
            && seen.insert(parent)
        {
            if out.len() - ancestor_start < 4 {
                out.push(parent);
            } else {
                remaining.push(parent);
            }
        }
    }
    out.extend_from_slice(fine);
    out.extend_from_slice(remaining);
    out.extend(
        fine.iter()
            .copied()
            .filter_map(|coords| missing_dem_fallback(loaded, coords, minzoom)),
    );
}

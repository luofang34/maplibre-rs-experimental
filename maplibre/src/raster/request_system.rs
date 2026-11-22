//! Requests tiles which are currently in view

mod memo;

use std::{borrow::Cow, marker::PhantomData, rc::Rc};

use rustc_hash::FxHashMap;

use crate::{
    context::MapContext,
    coords::WorldTileCoords,
    environment::Environment,
    io::{
        apc::{AsyncProcedureCall, Input, ProcedureTag},
        frame_style::FrameStyle,
        tile_backpressure::request_budget,
        tile_retry::{self, RequestKind},
        tile_sources::{missing_tile_fallback, RasterSourceId},
    },
    kernel::Kernel,
    raster::{transferables::RasterTransferables, RasterLayersDataComponent},
    render::{projection::raster_source_regions, view_state::ViewStatePadding},
    style::Style,
    tcs::system::{System, SystemError, SystemResult},
};

pub struct RequestSystem<E: Environment, T: RasterTransferables> {
    kernel: Rc<Kernel<E>>,
    memo: memo::RasterRequestMemo,
    phantom_t: PhantomData<T>,
}

impl<E: Environment, T: RasterTransferables> RequestSystem<E, T> {
    pub fn new(kernel: &Rc<Kernel<E>>) -> Self {
        Self {
            kernel: kernel.clone(),
            memo: memo::RasterRequestMemo::default(),
            phantom_t: Default::default(),
        }
    }
}

impl<E: Environment, T: RasterTransferables> System for RequestSystem<E, T> {
    fn name(&self) -> Cow<'static, str> {
        "raster_request".into()
    }

    fn run(
        &mut self,
        MapContext {
            style,
            view_state,
            world,
            renderer,
        }: &mut MapContext,
    ) -> SystemResult {
        tile_retry::stop_cancelled(world, self.kernel.apc());
        crate::io::tile_backpressure::set_raster_deferred(world, false);
        let (has_raster_layers, sources_empty) = self.memo.sync_style(style);
        if !has_raster_layers || sources_empty {
            self.memo.invalidate();
            return Ok(());
        }
        let multi = world
            .resources
            .get::<crate::render::eye_covering::MultiViewExtractCovering>()
            .filter(|m| m.active)
            .cloned();
        let tight_padding = renderer.settings.tight_tile_requests
            || (cfg!(target_arch = "wasm32")
                && (!view_state.has_external_view() || view_state.request_overscan() <= 1.0));
        let memo_key = memo::RasterRequestMemo::build_key(
            world,
            style,
            view_state,
            tight_padding,
            multi.as_ref(),
        );
        if self.memo.try_reuse(world, &memo_key) {
            return Ok(());
        }
        let all_sources = self.memo.cached_sources();
        let (mut regions, cached_regions) =
            resolve_regions(style, view_state, world, tight_padding, multi.as_ref())?;
        regions.retain(|(source, _)| {
            !crate::raster::media_source::is_media_source(style, source.name())
        });
        let stand_ins = drape_stand_ins(style, view_state, world);
        self.dispatch_wanted(
            style,
            world,
            &all_sources,
            regions,
            stand_ins,
            multi.as_ref(),
        );
        self.memo
            .record(memo_key, world.tiles.tiles.len(), cached_regions);
        Ok(())
    }
}

type ResolvedRegions = (
    crate::render::eye_covering::RasterCoverings,
    Option<(
        crate::render::eye_covering::RasterCoverings,
        ViewStatePadding,
        (u64, u64, usize),
    )>,
);

fn resolve_regions(
    style: &Style,
    view_state: &crate::render::view_state::ViewState,
    world: &mut crate::tcs::world::World,
    tight_padding: bool,
    multi: Option<&crate::render::eye_covering::MultiViewExtractCovering>,
) -> Result<ResolvedRegions, SystemError> {
    if let Some(multi) = multi {
        return Ok((multi.raster_regions.clone(), None));
    }
    let request_padding = if tight_padding {
        ViewStatePadding::Tight
    } else {
        ViewStatePadding::Loose
    };
    let computed = raster_source_regions(style, view_state, world, request_padding)?;
    let host_frame = crate::render::frame_signals::host_frame_in_progress(world);
    let terrain_key = crate::render::eye_covering::terrain_index_key(world);
    let cached_regions = computed.clone();
    let cache = world
        .resources
        .get_or_init_mut::<crate::render::eye_covering::FrameLooseCovering>();
    if cache.host_frame != host_frame {
        *cache = crate::render::eye_covering::FrameLooseCovering::for_host_frame(host_frame);
    }
    cache.raster_regions = Some((cached_regions.clone(), request_padding, terrain_key));
    Ok((
        computed,
        Some((cached_regions, request_padding, terrain_key)),
    ))
}

impl<E: Environment, T: RasterTransferables> RequestSystem<E, T> {
    fn dispatch_wanted(
        &mut self,
        style: &Style,
        world: &mut crate::tcs::world::World,
        all_sources: &[RasterSourceId],
        regions: crate::render::eye_covering::RasterCoverings,
        stand_ins: Vec<WorldTileCoords>,
        multi: Option<&crate::render::eye_covering::MultiViewExtractCovering>,
    ) {
        let mut requested = std::mem::take(&mut self.memo.requested_scratch);
        requested.clear();
        let mut budget = request_budget(world);
        let mut acc = std::mem::take(&mut self.memo.acc);
        wanted_tiles_with_sources_into(&mut acc, regions, stand_ins, style, world);
        let mut deferred = false;
        let mut frame_style = FrameStyle::new(style);
        let wanted = std::mem::take(&mut acc.wanted);
        for coords in &wanted {
            let coords = *coords;
            if !requested.insert(coords) {
                continue;
            }
            let retry_due = tile_retry::due(world, coords, RequestKind::Raster);
            if tile_retry::waiting(world, coords, RequestKind::Raster) && !retry_due {
                continue;
            }
            let target_sources = acc
                .sources_by_coord
                .get(&coords)
                .map_or(all_sources, Vec::as_slice);
            let pending_sources: Vec<&RasterSourceId> = target_sources
                .iter()
                .filter(|source| retry_due || !source_ready_at(world, source, coords))
                .collect();
            if pending_sources.is_empty() {
                continue;
            }
            if budget == 0 {
                deferred = true;
                continue;
            }
            budget -= 1;
            let only_sources = pending_sources
                .iter()
                .map(|source| source.name().map(str::to_owned))
                .collect::<Option<Vec<String>>>();
            if let Err(error) = self.request(coords, only_sources, &mut frame_style, world) {
                tracing::warn!(%error, "tile request rejected; it is requested again after a backoff");
                tile_retry::rejected(world, coords, RequestKind::Raster, None);
            }
        }
        acc.wanted = wanted;
        self.memo.acc = acc;
        crate::io::tile_backpressure::set_raster_deferred(world, deferred);
        if let Some(multi) = multi {
            requested.extend(multi.skipped_wanted_raster.iter().copied());
        }
        tile_retry::want(world, RequestKind::Raster, &requested);
        self.memo.requested_scratch = requested;
    }
    fn request(
        &self,
        coords: crate::coords::WorldTileCoords,
        only_sources: Option<Vec<String>>,
        style: &mut FrameStyle,
        world: &mut crate::tcs::world::World,
    ) -> SystemResult {
        let exists = world
            .tiles
            .query::<&RasterLayersDataComponent>(coords)
            .is_some();
        let attempt = tile_retry::next_attempt(world);
        let Some(mut tile) = world.tiles.spawn_mut(coords) else {
            return Err(SystemError::InvalidTile { coords });
        };
        let mut input = Input::new(coords, style.worker_snapshot(), attempt);
        input.only_sources = only_sources;
        self.kernel
            .apc()
            .call_tagged(
                ProcedureTag::FetchRaster,
                input,
                fetch_raster_apc::<E::OffscreenKernelEnvironment, T, _>,
            )
            .map_err(|source| SystemError::TileRequest {
                kind: "raster",
                coords,
                source,
            })?;
        if !exists {
            tile.insert(RasterLayersDataComponent::default());
        }
        tile_retry::started(world, coords, RequestKind::Raster, attempt, None);
        tracing::debug!(%coords, "raster tile request accepted");
        Ok(())
    }
}

fn source_ready_at(
    world: &crate::tcs::world::World,
    source: &RasterSourceId,
    coords: WorldTileCoords,
) -> bool {
    if world
        .tiles
        .query::<&crate::terrain::DemLayersComponent>(coords)
        .is_some_and(|component| component.contains(source))
    {
        return true;
    }
    let Some(component) = world.tiles.query::<&RasterLayersDataComponent>(coords) else {
        return false;
    };
    let Some(layer) = component.layers.iter().find(|l| l.source() == source) else {
        return false;
    };
    match layer {
        crate::raster::RasterLayerData::Missing(_) | crate::raster::RasterLayerData::Empty(_) => {
            true
        }
        crate::raster::RasterLayerData::Available(avail) if !avail.image.is_empty() => true,
        crate::raster::RasterLayerData::Available(_) => match world
            .resources
            .get::<crate::render::eventually::Eventually<
            crate::raster::resource::RasterResources,
        >>() {
            Some(crate::render::eventually::Eventually::Initialized(resources))
                if resources.draws(source) =>
            {
                resources.get_bound_texture(source, &coords).is_some()
            }
            _ => true,
        },
    }
}

fn record_wanted_source(
    map: &mut FxHashMap<WorldTileCoords, Vec<RasterSourceId>>,
    coords: WorldTileCoords,
    source: &RasterSourceId,
) {
    let list = map.entry(coords).or_default();
    if !list.contains(source) {
        list.push(source.clone());
    }
}

fn coarse_raster_ancestor(coords: WorldTileCoords, minzoom: u8) -> Option<WorldTileCoords> {
    let target = u8::from(coords.z).saturating_sub(3).max(minzoom);
    if target >= u8::from(coords.z) {
        return None;
    }
    let mut current = coords;
    while u8::from(current.z) > target {
        current = current.get_parent()?;
    }
    Some(current)
}

/// The terrain tiles an eye's drapes are drawn into, nearest first; none for the map's own
/// camera, whose drapes are as fine as its view tiles and need no coarser stand-in.
fn drape_stand_ins(
    style: &Style,
    view_state: &crate::render::view_state::ViewState,
    world: &crate::tcs::world::World,
) -> Vec<WorldTileCoords> {
    if style.terrain.is_none()
        || !(view_state.has_external_view() || view_state.pose_view().is_some())
    {
        return Vec::new();
    }
    world
        .resources
        .get::<crate::terrain::request_system::DrapeRequests>()
        .map(|requests| requests.0.clone())
        .unwrap_or_default()
}

/// The tile of a source that fills a terrain tile by itself: the tile at its coordinates, or
/// its ancestor at the deepest level the source serves.
fn stand_in_tile(target: WorldTileCoords, minzoom: u8, maxzoom: u8) -> Option<WorldTileCoords> {
    if u8::from(target.z) < minzoom {
        return None;
    }
    let mut current = target;
    while u8::from(current.z) > maxzoom {
        current = current.get_parent()?;
    }
    Some(current)
}

/// The tiles to request, most urgent first.
///
/// An eye in flight keeps wanting new tiles near it, and a terrain tile's drape has nothing of
/// its own to show until a tile covering all of it has loaded. Each of `drape_targets` is
/// therefore requested from every source before any finer tile, so the ground out to the
/// horizon holds imagery while the detail streams in behind it.
#[cfg(test)]
fn wanted_tiles(
    regions: Vec<(RasterSourceId, Vec<WorldTileCoords>)>,
    drape_targets: Vec<WorldTileCoords>,
    style: &crate::style::Style,
    world: &crate::tcs::world::World,
) -> Vec<WorldTileCoords> {
    let mut acc = memo::WantedTilesAcc::default();
    wanted_tiles_with_sources_into(&mut acc, regions, drape_targets, style, world);
    acc.wanted
}

use memo::WantedTilesAcc;

type SourceTileConstraints = (u8, Option<(f64, f64, f64, f64)>, u8);

fn source_tile_constraints(
    source: &RasterSourceId,
    style: &crate::style::Style,
) -> SourceTileConstraints {
    let minzoom = match source.name().and_then(|name| style.sources.get(name)) {
        Some(crate::style::source::Source::Raster(source)) => source.minzoom,
        Some(crate::style::source::Source::RasterDem(source)) => source.minzoom,
        _ => None,
    }
    .unwrap_or(0);
    let bounds = match source.name().and_then(|name| style.sources.get(name)) {
        Some(crate::style::source::Source::Raster(source)) => source.bounds,
        Some(crate::style::source::Source::Image(image)) => image_bounds(image),
        _ => None,
    };
    let maxzoom = match source.name().and_then(|name| style.sources.get(name)) {
        Some(crate::style::source::Source::Raster(source)) => source.maxzoom,
        Some(crate::style::source::Source::RasterDem(source)) => source.maxzoom,
        _ => None,
    }
    .unwrap_or(u8::MAX);
    (minzoom, bounds, maxzoom)
}

fn collect_source_wanted_tiles(
    acc: &mut WantedTilesAcc,
    source: &RasterSourceId,
    tiles: Vec<WorldTileCoords>,
    drape_targets: &[WorldTileCoords],
    style: &crate::style::Style,
    world: &crate::tcs::world::World,
) {
    let is_terrain = style.terrain.is_some();
    let (minzoom, bounds, maxzoom) = source_tile_constraints(source, style);
    if !tiles.is_empty() {
        for coords in drape_targets
            .iter()
            .filter_map(|target| stand_in_tile(*target, minzoom, maxzoom))
            .filter(|coords| {
                bounds.is_none_or(|bounds| crate::io::tile_sources::tile_in_bounds(*coords, bounds))
            })
        {
            record_wanted_source(&mut acc.sources_by_coord, coords, source);
            if acc.seen_stand_ins.insert(coords) {
                acc.stand_ins.push(coords);
            }
        }
    }
    for coords in tiles {
        // Tiles outside the source's declared bounds are never requested.
        if bounds.is_some_and(|bounds| !crate::io::tile_sources::tile_in_bounds(coords, bounds)) {
            continue;
        }
        if is_terrain {
            if let Some(ancestor) = coarse_raster_ancestor(coords, minzoom) {
                if acc.seen_coarse.insert(ancestor) {
                    acc.coarse_ancestors.push(ancestor);
                }
                if !source_ready_at(world, source, coords) {
                    acc.needed_coarse.insert(ancestor);
                    record_wanted_source(&mut acc.sources_by_coord, ancestor, source);
                }
            }
        }
        record_wanted_source(&mut acc.sources_by_coord, coords, source);
        acc.wanted.push(coords);
        if let Some(parent) = missing_tile_fallback(coords, minzoom, |coords| {
            world
                .tiles
                .query::<&RasterLayersDataComponent>(coords)
                .is_some_and(|component| component.source_is_missing(source))
        }) {
            record_wanted_source(&mut acc.sources_by_coord, parent, source);
            acc.wanted.push(parent);
        }
    }
}

fn wanted_tiles_with_sources_into(
    acc: &mut WantedTilesAcc,
    regions: Vec<(RasterSourceId, Vec<WorldTileCoords>)>,
    drape_targets: Vec<WorldTileCoords>,
    style: &crate::style::Style,
    world: &crate::tcs::world::World,
) {
    acc.reset();
    for (source, tiles) in regions {
        collect_source_wanted_tiles(acc, &source, tiles, &drape_targets, style, world);
    }
    acc.coarse_ancestors
        .retain(|ancestor| acc.needed_coarse.contains(ancestor));
    if !acc.coarse_ancestors.is_empty() {
        // Bracket the local terrain with one coarse raster overview tile first, then fine
        // tiles, then any remaining coarse ancestors, matching DEM `request_order`.
        let remaining = acc
            .coarse_ancestors
            .split_off(acc.coarse_ancestors.len().min(2));
        acc.coarse_ancestors.append(&mut acc.wanted);
        acc.coarse_ancestors.extend(remaining);
        acc.stand_ins.append(&mut acc.coarse_ancestors);
        std::mem::swap(&mut acc.stand_ins, &mut acc.wanted);
        return;
    }
    acc.stand_ins.append(&mut acc.wanted);
    std::mem::swap(&mut acc.stand_ins, &mut acc.wanted);
}

/// The bounds `(west, south, east, north)` of an image's corners, so tiles it cannot reach are
/// not requested; none for a picture that reaches past the antimeridian, whose copies across it
/// a tile may show.
fn image_bounds(image: &crate::style::source::ImageSource) -> Option<(f64, f64, f64, f64)> {
    let longitudes = image.coordinates.map(|[longitude, _]| longitude);
    let latitudes = image.coordinates.map(|[_, latitude]| latitude);
    let fold = |values: [f64; 4]| {
        values
            .into_iter()
            .fold((f64::MAX, f64::MIN), |(lo, hi), value| {
                (lo.min(value), hi.max(value))
            })
    };
    let ((west, east), (south, north)) = (fold(longitudes), fold(latitudes));
    (west >= -180.0 && east <= 180.0).then_some((west, south, east, north))
}

mod fetch;
pub use fetch::fetch_raster_apc;

#[cfg(test)]
mod tests;

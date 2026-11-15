//! Requests tiles which are currently in view

use std::{borrow::Cow, marker::PhantomData, rc::Rc};

use rustc_hash::{FxHashMap, FxHashSet};

use crate::{
    context::MapContext,
    coords::WorldTileCoords,
    environment::Environment,
    io::{
        apc::{AsyncProcedureCall, Input},
        frame_style::FrameStyle,
        tile_backpressure::request_budget,
        tile_retry::{self, RequestKind},
        tile_sources::{missing_tile_fallback, source_layer_groups, TileKind},
    },
    kernel::Kernel,
    raster::{transferables::RasterTransferables, RasterLayersDataComponent, RasterSourceId},
    render::{projection::raster_source_regions, view_state::ViewStatePadding},
    style::Style,
    tcs::system::{System, SystemError, SystemResult},
};

pub struct RequestSystem<E: Environment, T: RasterTransferables> {
    kernel: Rc<Kernel<E>>,
    phantom_t: PhantomData<T>,
}

impl<E: Environment, T: RasterTransferables> RequestSystem<E, T> {
    pub fn new(kernel: &Rc<Kernel<E>>) -> Self {
        Self {
            kernel: kernel.clone(),
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
        // Missing ancestors and deferred tiles must progress while the camera is stationary.
        // Each raster source covers the view at its own tile size and rounding, as GL JS's
        // per-source tile managers do; the tiles of every source are requested together.
        let multi = world
            .resources
            .get::<crate::render::eye_covering::MultiViewExtractCovering>()
            .filter(|m| m.active)
            .cloned();
        let mut regions = if let Some(multi) = &multi {
            multi.raster_regions.clone()
        } else {
            let request_padding = if renderer.settings.tight_tile_requests
                || (cfg!(target_arch = "wasm32")
                    && style.terrain.is_none()
                    && !view_state.has_external_view())
            {
                ViewStatePadding::Tight
            } else {
                ViewStatePadding::Loose
            };
            let computed = raster_source_regions(style, view_state, world, request_padding)?;
            let host_frame = crate::render::frame_signals::host_frame_in_progress(world);
            let flat: Vec<WorldTileCoords> = computed
                .iter()
                .flat_map(|(_, tiles)| tiles.iter().copied())
                .collect();
            let cache = world
                .resources
                .get_or_init_mut::<crate::render::eye_covering::FrameLooseCovering>();
            if cache.host_frame != host_frame {
                *cache = crate::render::eye_covering::FrameLooseCovering {
                    host_frame,
                    view_tiles: None,
                    raster_tiles: None,
                };
            }
            cache.raster_tiles = Some(flat);
            computed
        };
        regions.retain(|(source, _)| {
            !crate::raster::media_source::is_media_source(style, source.name())
        });
        let mut requested = FxHashSet::default();
        let mut budget = request_budget(world);
        let stand_ins = drape_stand_ins(style, view_state, world);
        let (wanted, wanted_sources) = wanted_tiles_with_sources(regions, stand_ins, style, world);
        let all_sources: Vec<_> = source_layer_groups(style, TileKind::Raster)
            .into_iter()
            .filter(|group| {
                !crate::raster::media_source::is_media_source(style, group.source_name.as_deref())
            })
            .map(|group| RasterSourceId::new(group.source_name))
            .collect();
        let mut deferred = false;
        let mut frame_style = FrameStyle::new(style);
        for coords in wanted {
            if !requested.insert(coords) {
                continue;
            }

            let retry_due = tile_retry::due(world, coords, RequestKind::Raster);
            if tile_retry::waiting(world, coords, RequestKind::Raster) && !retry_due {
                continue;
            }
            let target_sources = wanted_sources
                .get(&coords)
                .map_or(all_sources.as_slice(), Vec::as_slice);
            let pending_sources: Vec<&RasterSourceId> = target_sources
                .iter()
                .filter(|source| retry_due || !source_ready_at(world, source, coords))
                .collect();
            if pending_sources.is_empty() {
                continue;
            }
            // The rest wait for a later frame, once tiles in flight have landed.
            if budget == 0 {
                deferred = true;
                continue;
            }
            budget -= 1;

            let only_sources = pending_sources
                .iter()
                .map(|source| source.name().map(str::to_owned))
                .collect::<Option<Vec<String>>>();
            // One rejected tile must not cost the frame its other tiles.
            if let Err(error) = self.request(coords, only_sources, &mut frame_style, world) {
                tracing::warn!(%error, "tile request rejected; it is requested again after a backoff");
                tile_retry::rejected(world, coords, RequestKind::Raster, None);
            }
        }
        crate::io::tile_backpressure::set_raster_deferred(world, deferred);
        if let Some(multi) = &multi {
            requested.extend(multi.skipped_wanted_raster.iter().copied());
        }
        tile_retry::want(world, RequestKind::Raster, &requested);

        Ok(())
    }
}

impl<E: Environment, T: RasterTransferables> RequestSystem<E, T> {
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
            .call(
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
        crate::raster::RasterLayerData::Missing(_) => true,
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
    wanted_tiles_with_sources(regions, drape_targets, style, world).0
}

fn wanted_tiles_with_sources(
    regions: Vec<(RasterSourceId, Vec<WorldTileCoords>)>,
    drape_targets: Vec<WorldTileCoords>,
    style: &crate::style::Style,
    world: &crate::tcs::world::World,
) -> (
    Vec<WorldTileCoords>,
    FxHashMap<WorldTileCoords, Vec<RasterSourceId>>,
) {
    let mut stand_ins = Vec::new();
    let mut seen_stand_ins = FxHashSet::default();
    let mut wanted = Vec::new();
    let mut coarse_ancestors = Vec::new();
    let mut seen_coarse = FxHashSet::default();
    let mut needed_coarse = FxHashSet::default();
    let mut sources_by_coord: FxHashMap<WorldTileCoords, Vec<RasterSourceId>> =
        FxHashMap::default();
    let is_terrain = style.terrain.is_some();
    for (source, tiles) in regions {
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
        if !tiles.is_empty() {
            for coords in drape_targets
                .iter()
                .filter_map(|target| stand_in_tile(*target, minzoom, maxzoom))
                .filter(|coords| {
                    bounds.is_none_or(|bounds| {
                        crate::io::tile_sources::tile_in_bounds(*coords, bounds)
                    })
                })
            {
                record_wanted_source(&mut sources_by_coord, coords, &source);
                if seen_stand_ins.insert(coords) {
                    stand_ins.push(coords);
                }
            }
        }
        for coords in tiles {
            // Tiles outside the source's declared bounds are never requested.
            if bounds.is_some_and(|bounds| !crate::io::tile_sources::tile_in_bounds(coords, bounds))
            {
                continue;
            }
            if is_terrain {
                if let Some(ancestor) = coarse_raster_ancestor(coords, minzoom) {
                    if seen_coarse.insert(ancestor) {
                        coarse_ancestors.push(ancestor);
                    }
                    if !source_ready_at(world, &source, coords) {
                        needed_coarse.insert(ancestor);
                        record_wanted_source(&mut sources_by_coord, ancestor, &source);
                    }
                }
            }
            record_wanted_source(&mut sources_by_coord, coords, &source);
            wanted.push(coords);
            if let Some(parent) = missing_tile_fallback(coords, minzoom, |coords| {
                world
                    .tiles
                    .query::<&RasterLayersDataComponent>(coords)
                    .is_some_and(|component| component.source_is_missing(&source))
            }) {
                record_wanted_source(&mut sources_by_coord, parent, &source);
                wanted.push(parent);
            }
        }
    }
    coarse_ancestors.retain(|ancestor| needed_coarse.contains(ancestor));
    if !coarse_ancestors.is_empty() {
        // Bracket the local terrain with one coarse raster overview tile first, then fine
        // tiles, then any remaining coarse ancestors, matching DEM `request_order`.
        let remaining = coarse_ancestors.split_off(coarse_ancestors.len().min(2));
        coarse_ancestors.extend(wanted);
        coarse_ancestors.extend(remaining);
        stand_ins.extend(coarse_ancestors);
        return (stand_ins, sources_by_coord);
    }
    stand_ins.extend(wanted);
    (stand_ins, sources_by_coord)
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

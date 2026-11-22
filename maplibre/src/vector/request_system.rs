//! Requests tiles which are currently in view

use std::{borrow::Cow, marker::PhantomData, rc::Rc, sync::Arc};

use rustc_hash::FxHashSet;

use crate::{
    context::MapContext,
    environment::Environment,
    io::{
        frame_style::FrameStyle,
        tile_backpressure::vector_request_budget,
        tile_retry::{self, RequestKind},
        tile_sources::{outside_source_bounds, source_max_zoom, source_min_zoom, TileKind},
    },
    kernel::Kernel,
    render::{tile_view_pattern::DEFAULT_TILE_SIZE, view_state::ViewStatePadding},
    tcs::system::{System, SystemResult},
    vector::{transferables::VectorTransferables, VectorLayerBucketComponent},
};

mod dispatch;
mod fallback;
mod group_tile;
mod limiter;
mod memo;
pub mod rescale;
mod velocity;
mod worker;
pub use dispatch::depends_on_overscaling;
use dispatch::request_tile;
#[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
pub(crate) use limiter::{acquire_heavy_tile_slot, PendingUploadPermits, SharedTilePermit};
pub(crate) use limiter::{
    clear_upload_permits, reconcile_upload_permits, snapshot_permit_sources, SharedTileClaim,
    SharedTileSlot,
};
use memo::{
    display_pixel_ratio, resolve_view_tiles, update_loose_covering_cache,
    vector_eviction_generation, RequestMemo, RequestViewKey, ResolvedViewTiles,
    ViewTileResolutionRequest,
};
pub use worker::fetch_vector_apc;

pub struct RequestSystem<E: Environment, T> {
    kernel: Rc<Kernel<E>>,
    phantom_t: PhantomData<T>,
    requested: FxHashSet<crate::coords::WorldTileCoords>,
    memo: Option<RequestMemo>,
}

impl<E: Environment, T> RequestSystem<E, T> {
    pub fn new(kernel: &Rc<Kernel<E>>) -> Self {
        Self {
            kernel: kernel.clone(),
            phantom_t: Default::default(),
            requested: FxHashSet::default(),
            memo: None,
        }
    }
}

impl<E: Environment, T: VectorTransferables> System for RequestSystem<E, T> {
    fn name(&self) -> Cow<'static, str> {
        "vector_request".into()
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
        crate::io::tile_backpressure::set_vector_deferred(world, false);
        let vectors_disabled = renderer.settings.buffer_pools.is_empty();
        let symbols_disabled = renderer.settings.symbol_pools.is_empty();
        if vectors_disabled && symbols_disabled {
            return Ok(());
        }
        let pixel_ratio = display_pixel_ratio(world, renderer, view_state);
        let multi = world
            .resources
            .get::<crate::render::eye_covering::MultiViewExtractCovering>()
            .filter(|m| m.active)
            .cloned();
        let visible_level = view_state.zoom().zoom_level(DEFAULT_TILE_SIZE);
        let request_padding = if renderer.settings.tight_tile_requests
            || (cfg!(target_arch = "wasm32") && !view_state.has_external_view())
        {
            ViewStatePadding::Tight
        } else {
            ViewStatePadding::Loose
        };
        let drapes = world
            .resources
            .get::<crate::terrain::request_system::DrapeRequests>()
            .map(|requests| requests.0.clone())
            .unwrap_or_default();
        let prefetch = world
            .resources
            .get::<crate::terrain::request_system::DrapePrefetchRequests>()
            .map(|requests| requests.0.clone())
            .unwrap_or_default();
        let vector_evictions = vector_eviction_generation(world);
        let key = RequestViewKey::build(
            world,
            style,
            view_state,
            renderer,
            pixel_ratio,
            request_padding,
            visible_level,
        );
        let settled_unchanged = key.as_ref().is_some_and(|key| {
            self.memo.as_ref().is_some_and(|memo| {
                memo.settled
                    && memo.drapes == drapes
                    && memo.prefetch == prefetch
                    && memo.vector_evictions == vector_evictions
                    && memo.key.matches_non_matrix(key)
                    && !crate::sdf::provided::awaiting(world)
                    && crate::io::tile_backpressure::tiles_in_flight(world) == 0
                    && !tile_retry::needs_frame(world)
                    && tile_retry::next_retry_delay(world).is_none()
            })
        });
        if multi.is_none() && settled_unchanged {
            if let (Some(key), Some(memo)) = (key.as_ref(), self.memo.as_ref()) {
                if memo.key == *key {
                    tile_retry::want(world, RequestKind::Vector, &self.requested);
                    release_unwanted_tiles(world, &self.requested);
                    update_loose_covering_cache(
                        world,
                        memo.computed.as_ref(),
                        request_padding,
                        visible_level,
                    );
                    return Ok(());
                }
            }
        }

        let draped = style.terrain.is_some();
        let derived =
            style.cached_derived_facts(|style| crate::style::revision::DerivedStyleFacts {
                depends_on_overscaling: depends_on_overscaling(style),
                elevated_headroom_meters: crate::render::projection::style_elevated_headroom_meters(
                    style,
                ),
            });
        // View-space 3D volumes (`fill-extrusion`) and elevated 3D paths (`line-elevation`)
        // draw from `view_tiles` (which carry vertical headroom) rather than terrain drapes.
        let needs_view_tiles = !draped
            || derived.elevated_headroom_meters > 0.0
            || (!renderer.settings.terrain_drape_mode.is_procedural_relief() && drapes.is_empty());
        let magnifies = derived.depends_on_overscaling;

        let (view_tiles_and_level, tight_covering, memo_computed, globe_anchor) =
            if let Some(multi) = &multi {
                (
                    Some((
                        Arc::from(multi.vector_tiles.as_slice()),
                        multi.max_view_level,
                    )),
                    false,
                    None,
                    None,
                )
            } else if !needs_view_tiles {
                // Terrain drapes drive vector requests via `drapes`/`prefetch`; skip `view_region`
                // without overwriting `FrameLooseCovering.view_tiles`.
                (
                    Some((Arc::from([]), u8::from(visible_level))),
                    request_padding == ViewStatePadding::Tight,
                    None,
                    None,
                )
            } else {
                let resolved = resolve_view_tiles(ViewTileResolutionRequest {
                    memo: self.memo.as_ref(),
                    key: key.as_ref(),
                    settled_unchanged,
                    style,
                    view_state,
                    world,
                    visible_level,
                    request_padding,
                    tight_tile_requests: renderer.settings.tight_tile_requests,
                })?;
                let (computed, next_anchor) = match resolved {
                    ResolvedViewTiles::SettledReuse => {
                        let cached = self.memo.as_ref().and_then(|memo| memo.computed.as_ref());
                        tile_retry::want(world, RequestKind::Vector, &self.requested);
                        release_unwanted_tiles(world, &self.requested);
                        update_loose_covering_cache(world, cached, request_padding, visible_level);
                        return Ok(());
                    }
                    ResolvedViewTiles::Computed {
                        computed,
                        globe_anchor,
                    } => (computed, globe_anchor),
                };
                update_loose_covering_cache(
                    world,
                    computed.as_ref(),
                    request_padding,
                    visible_level,
                );
                (
                    computed.clone(),
                    request_padding == ViewStatePadding::Tight,
                    computed,
                    next_anchor,
                )
            };

        // Tile arrivals, eviction and a settling eye can change the covering without motion.
        if let Some((view_tiles, level)) = view_tiles_and_level {
            let max_zoom = source_max_zoom(style, TileKind::Vector);
            let min_zoom = source_min_zoom(style, TileKind::Vector);
            self.requested.clear();
            let mut budget = vector_request_budget(world, draped);

            let overview = draped.then_some(crate::coords::WorldTileCoords {
                x: 0,
                y: 0,
                z: crate::coords::ZoomLevel::new(0),
            });
            let extra_fallbacks = fallback::plan_non_terrain_fallbacks(
                style,
                view_state,
                world,
                (
                    renderer.settings.tight_tile_requests,
                    multi.is_some(),
                    tight_covering,
                ),
                &view_tiles,
                (level, magnifies),
            );
            let (coarse_fallbacks, lookahead_fallbacks): (Vec<_>, Vec<_>) = extra_fallbacks
                .into_iter()
                .partition(|coords| u8::from(coords.z) < level);
            let mut deferred = false;
            let mut issued_any = false;
            let mut frame_style = FrameStyle::new(style);
            for coords in overview
                .into_iter()
                .chain(drapes.iter().copied())
                .chain(coarse_fallbacks)
                .chain(view_tiles.iter().copied().filter(|_| needs_view_tiles))
                .chain(prefetch.iter().copied())
                .chain(lookahead_fallbacks)
            {
                // Past the deepest source zoom the ancestor tile is fetched once and the view
                // pattern scales it into every descendant in view; a source that ends sooner
                // is cut into the tile from its own last tile by the worker.
                if min_zoom.is_some_and(|min_zoom| u8::from(coords.z) < min_zoom) {
                    continue;
                }
                let Some(target) =
                    crate::coords::OverscaledTileId::for_source(coords, level, max_zoom, magnifies)
                else {
                    continue;
                };
                let coords = target.canonical;
                if outside_source_bounds(style, TileKind::Vector, coords)
                    || !self.requested.insert(coords)
                {
                    continue;
                }

                // A source's last zoom is drawn magnified at the zooms past it, and what depends on
                // that is laid out again when the magnification changes.
                let overscaled_zoom =
                    if magnifies && max_zoom.is_some_and(|m| u8::from(coords.z) >= m) {
                        target.overscaled_z
                    } else {
                        0
                    };
                let laid_out_for = (magnifies.then_some(overscaled_zoom), pixel_ratio);
                let evicted_gpu = !tile_retry::is_pending(world, coords, RequestKind::Vector)
                    && super::content::has_evicted_gpu_geometry(
                        world,
                        style,
                        (vectors_disabled, symbols_disabled),
                        coords,
                    );
                if tile_retry::backing_off(world, coords, RequestKind::Vector)
                    || (!evicted_gpu && is_current(world, coords, laid_out_for))
                {
                    continue;
                }
                // The rest wait for a later frame, once tiles in flight have landed.
                if budget == 0 {
                    deferred = true;
                    continue;
                }
                budget -= 1;
                issued_any = true;

                let only_sources = if evicted_gpu {
                    drop(tile_retry::take_refresh_sources(world, coords));
                    None
                } else {
                    refresh_only(world, coords, laid_out_for)
                };
                // One rejected tile must not cost the frame its other tiles.
                if let Err(error) = request_tile::<E, T>(
                    &self.kernel,
                    coords,
                    &mut frame_style,
                    world,
                    (overscaled_zoom, pixel_ratio),
                    only_sources.clone(),
                    (vectors_disabled, symbols_disabled),
                ) {
                    tracing::warn!(%error, "tile request rejected; it is requested again after a backoff");
                    tile_retry::rejected(
                        world,
                        coords,
                        RequestKind::Vector,
                        only_sources.as_deref(),
                    );
                }
            }
            crate::io::tile_backpressure::set_vector_deferred(world, deferred);
            if let Some(multi) = &multi {
                self.requested
                    .extend(multi.skipped_wanted_vector.iter().copied());
            }
            tile_retry::want(world, RequestKind::Vector, &self.requested);
            release_unwanted_tiles(world, &self.requested);
            if let Some(key) = key.filter(|_| multi.is_none()) {
                let settled = !deferred
                    && !issued_any
                    && !crate::sdf::provided::awaiting(world)
                    && crate::io::tile_backpressure::tiles_in_flight(world) == 0
                    && !tile_retry::needs_frame(world)
                    && tile_retry::next_retry_delay(world).is_none();
                self.memo = Some(RequestMemo {
                    key,
                    computed: memo_computed,
                    globe_anchor,
                    drapes,
                    prefetch,
                    vector_evictions,
                    settled,
                });
            } else {
                self.memo = None;
            }
        } else {
            self.memo = None;
        }
        Ok(())
    }
}

/// Whether the tile holds what a request would bring: laid out for the zoom it is magnified to,
/// if that matters, and its provided images made for `pixel_ratio`, with no retry due.
fn is_current(
    world: &mut crate::tcs::world::World,
    coords: crate::coords::WorldTileCoords,
    (overscaled_zoom, pixel_ratio): (Option<u8>, f32),
) -> bool {
    let Some(tile) = world.tiles.query::<&VectorLayerBucketComponent>(coords) else {
        return false;
    };
    overscaled_zoom.is_none_or(|zoom| tile.overscaled_zoom == zoom)
        && !crate::sdf::provided::drawn_for_another_ratio(world, coords, pixel_ratio)
        && !tile_retry::due(world, coords, RequestKind::Vector)
}

/// The sources the request for the tile is limited to: the ones a refresh of their data alone
/// is pending for, when the tile otherwise holds what a request would bring and draws no
/// provided images, whose bookkeeping a partial request would lose. A request for any other
/// reason processes every source.
fn refresh_only(
    world: &mut crate::tcs::world::World,
    coords: crate::coords::WorldTileCoords,
    (overscaled_zoom, pixel_ratio): (Option<u8>, f32),
) -> Option<Vec<String>> {
    let sources = tile_retry::take_refresh_sources(world, coords)?;
    let tile = world.tiles.query::<&VectorLayerBucketComponent>(coords)?;
    let complete = tile.done
        && !tile.failed
        && overscaled_zoom.is_none_or(|zoom| tile.overscaled_zoom == zoom);
    (complete
        && !crate::sdf::provided::drawn_for_another_ratio(world, coords, pixel_ratio)
        && !crate::sdf::provided::uses_images(world, coords))
    .then_some(sources)
}

/// Queues the waiting workers of tiles no longer in view to stop with the frame's cancellations.
fn release_unwanted_tiles(
    world: &mut crate::tcs::world::World,
    wanted: &FxHashSet<crate::coords::WorldTileCoords>,
) {
    let attempts = crate::sdf::provided::release_unwanted(world, wanted);
    tile_retry::queue_cancelled(world, attempts);
}

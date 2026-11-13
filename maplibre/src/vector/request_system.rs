//! Requests tiles which are currently in view

use std::{borrow::Cow, marker::PhantomData, rc::Rc};

use rustc_hash::FxHashSet;

use crate::{
    context::MapContext,
    environment::Environment,
    io::{
        apc::{AsyncProcedureCall, Input},
        frame_style::FrameStyle,
        tile_backpressure::vector_request_budget,
        tile_retry::{self, RequestKind},
        tile_sources::{outside_source_bounds, source_max_zoom, source_min_zoom, TileKind},
    },
    kernel::Kernel,
    render::{
        projection::view_region_for_projection, tile_view_pattern::DEFAULT_TILE_SIZE,
        view_state::ViewStatePadding,
    },
    tcs::system::{System, SystemError, SystemResult},
    vector::{transferables::VectorTransferables, VectorLayerBucketComponent},
};

mod group_tile;
pub mod rescale;
mod worker;
pub use worker::fetch_vector_apc;

pub struct RequestSystem<E: Environment, T> {
    kernel: Rc<Kernel<E>>,
    phantom_t: PhantomData<T>,
}

impl<E: Environment, T> RequestSystem<E, T> {
    pub fn new(kernel: &Rc<Kernel<E>>) -> Self {
        Self {
            kernel: kernel.clone(),
            phantom_t: Default::default(),
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
        let view_tiles_and_level = if let Some(multi) = &multi {
            Some((multi.vector_tiles.clone(), multi.max_view_level))
        } else {
            let visible_level = view_state.zoom().zoom_level(DEFAULT_TILE_SIZE);
            let computed = view_region_for_projection(
                style,
                view_state,
                world,
                visible_level,
                ViewStatePadding::Loose,
            )?
            .map(|region| (region.iter().collect::<Vec<_>>(), u8::from(visible_level)));
            let host_frame = crate::render::frame_signals::host_frame_in_progress(world);
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
            cache.view_tiles = Some(
                computed
                    .as_ref()
                    .map_or_else(Vec::new, |(tiles, _)| tiles.clone()),
            );
            computed
        };

        // Tile arrivals, eviction and a settling eye can change the covering without motion.
        if let Some((view_tiles, level)) = view_tiles_and_level {
            let max_zoom = source_max_zoom(style, TileKind::Vector);
            let min_zoom = source_min_zoom(style, TileKind::Vector);
            let mut requested = FxHashSet::default();
            let mut budget = vector_request_budget(world, style.terrain.is_some());

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
            let overview = style
                .terrain
                .is_some()
                .then_some(crate::coords::WorldTileCoords {
                    x: 0,
                    y: 0,
                    z: crate::coords::ZoomLevel::new(0),
                });
            let draped = style.terrain.is_some();
            // View-space 3D volumes (`fill-extrusion`) and elevated 3D paths (`line-elevation`)
            // draw from `view_tiles` (which carry vertical headroom) rather than terrain drapes.
            let needs_view_tiles =
                !draped || crate::render::projection::style_elevated_headroom_meters(style) > 0.0;
            let magnifies = depends_on_overscaling(style);
            let mut frame_style = FrameStyle::new(style);
            for coords in overview
                .into_iter()
                .chain(drapes)
                .chain(view_tiles.into_iter().filter(|_| needs_view_tiles))
                .chain(prefetch)
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
                    || !requested.insert(coords)
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
                if tile_retry::backing_off(world, coords, RequestKind::Vector)
                    || is_current(world, coords, laid_out_for)
                {
                    continue;
                }
                // The rest wait for a later frame, once tiles in flight have landed.
                if budget == 0 {
                    continue;
                }
                budget -= 1;

                let only_sources = refresh_only(world, coords, laid_out_for);
                // One rejected tile must not cost the frame its other tiles.
                if let Err(error) = self.request(
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
            if let Some(multi) = &multi {
                requested.extend(multi.skipped_wanted_vector.iter().copied());
            }
            tile_retry::want(world, RequestKind::Vector, &requested);
            self.release_unwanted(world, &requested);
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

/// Device pixels per layout pixel of the display: as the host reported it, or else as the
/// surface and viewport sizes give it.
fn display_pixel_ratio(
    world: &crate::tcs::world::World,
    renderer: &crate::render::Renderer,
    view_state: &crate::render::view_state::ViewState,
) -> f32 {
    world
        .resources
        .get::<crate::sdf::provided::DisplayPixelRatio>()
        .map_or_else(
            || {
                pixel_ratio(
                    renderer.state().surface().size().width(),
                    view_state.width(),
                )
            },
            |ratio| ratio.0,
        )
}

/// Device pixels per layout pixel read from the surface and viewport, to the hundredth so that
/// a ratio computed from rounded sizes does not ask for images again.
fn pixel_ratio(physical_width: u32, logical_width: f64) -> f32 {
    let ratio = f64::from(physical_width) / logical_width.max(1.0);
    ((ratio * 100.0).round() / 100.0).clamp(0.25, 8.0) as f32
}

impl<E: Environment, T: VectorTransferables> RequestSystem<E, T> {
    /// Stops the workers that wait for images for tiles no longer in view.
    fn release_unwanted(
        &self,
        world: &mut crate::tcs::world::World,
        wanted: &FxHashSet<crate::coords::WorldTileCoords>,
    ) {
        for attempt in crate::sdf::provided::release_unwanted(world, wanted) {
            self.kernel.apc().cancel(attempt);
        }
    }

    fn request(
        &self,
        coords: crate::coords::WorldTileCoords,
        style: &mut FrameStyle,
        world: &mut crate::tcs::world::World,
        (overscaled_zoom, pixel_ratio): (u8, f32),
        only_sources: Option<Vec<String>>,
        (vectors_disabled, symbols_disabled): (bool, bool),
    ) -> SystemResult {
        if world.tiles.spawn_mut(coords).is_none() {
            return Err(SystemError::InvalidTile { coords });
        }
        let refetch = tile_retry::due(world, coords, RequestKind::Vector);
        let attempt = tile_retry::next_attempt(world);
        let issued_for = only_sources.clone();
        let worker_style = style.share(|style| {
            let mut worker_style = style.clone();
            if symbols_disabled {
                worker_style.layers.retain(|layer| layer.type_ != "symbol");
            }
            if vectors_disabled {
                worker_style.layers.retain(|layer| layer.type_ == "symbol");
            }
            worker_style
        });
        // What the worker is asked to build: its style's non-symbol layers, for the refreshed
        // sources alone when the request is limited to them.
        let requested: Vec<String> = worker_style
            .layers
            .iter()
            .filter(|layer| {
                layer.type_ != "symbol"
                    && crate::style::mutation::from_vector_tiles(layer)
                    && only_sources.as_ref().is_none_or(|sources| {
                        layer
                            .source
                            .as_ref()
                            .is_some_and(|source| sources.contains(source))
                    })
            })
            .map(|layer| layer.id.clone())
            .collect();
        self.kernel
            .apc()
            .call(
                Input {
                    coords,
                    style: worker_style,
                    attempt,
                    overscaled_zoom,
                    pixel_ratio,
                    only_sources,
                    refetch,
                },
                fetch_vector_apc::<E::OffscreenKernelEnvironment, T, _>,
            )
            .map_err(|source| SystemError::TileRequest {
                kind: "vector",
                coords,
                source,
            })?;
        super::content::begin(world, coords, requested);
        if let Some(tile) = world
            .tiles
            .query_mut::<&mut VectorLayerBucketComponent>(coords)
        {
            tile.overscaled_zoom = overscaled_zoom;
        }
        tile_retry::started(
            world,
            coords,
            RequestKind::Vector,
            attempt,
            issued_for.as_deref(),
        );
        if let Some(replaced) = crate::sdf::provided::restarted(world, coords, pixel_ratio) {
            self.kernel.apc().cancel(replaced);
        }
        tracing::debug!(%coords, "vector tile request accepted");
        Ok(())
    }
}

/// Whether tiles are laid out for the zoom they are magnified to: layout and filter
/// expressions are evaluated at that zoom, and labels along a line space themselves by it.
pub fn depends_on_overscaling(style: &crate::style::Style) -> bool {
    style.layers.iter().any(|layer| layer.paint.is_some())
}

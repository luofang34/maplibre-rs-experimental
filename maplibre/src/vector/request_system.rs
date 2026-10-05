//! Requests tiles which are currently in view

use std::{borrow::Cow, collections::HashSet, marker::PhantomData, rc::Rc};

use crate::{
    context::MapContext,
    environment::Environment,
    io::{
        apc::{AsyncProcedureCall, Input},
        tile_backpressure::vector_request_budget,
        tile_retry::{self, RequestKind},
        tile_sources::{
            clamp_to_max_zoom, outside_source_bounds, source_max_zoom, source_min_zoom, TileKind,
        },
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
        let pixel_ratio = display_pixel_ratio(world, renderer, view_state);
        let view_region = view_region_for_projection(
            style,
            view_state,
            world,
            view_state.zoom().zoom_level(DEFAULT_TILE_SIZE),
            ViewStatePadding::Loose,
        )?;

        // Tile arrivals, eviction and a settling eye can change the covering without motion.
        if let Some(view_region) = &view_region {
            let max_zoom = source_max_zoom(style, TileKind::Vector);
            let min_zoom = source_min_zoom(style, TileKind::Vector);
            let mut requested = HashSet::new();
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
            let magnifies = depends_on_overscaling(style);
            let level = u8::from(view_state.zoom().zoom_level(DEFAULT_TILE_SIZE));
            for coords in overview
                .into_iter()
                .chain(drapes)
                .chain(view_region.iter().filter(|_| !draped))
                .chain(prefetch)
            {
                // Above the source maximum zoom the ancestor tile is fetched once and the
                // view pattern scales it into every descendant in view.
                if min_zoom.is_some_and(|min_zoom| u8::from(coords.z) < min_zoom) {
                    continue;
                }
                let coords = clamp_to_max_zoom(coords, max_zoom);
                if coords.build_quad_key().is_none()
                    || outside_source_bounds(style, TileKind::Vector, coords)
                    || !requested.insert(coords)
                {
                    continue;
                }

                // A source's last zoom is drawn magnified at the zooms past it, and what depends on
                // that is laid out again when the magnification changes.
                let overscaled_zoom = match max_zoom {
                    Some(max_zoom) if magnifies && u8::from(coords.z) >= max_zoom => {
                        level.max(u8::from(coords.z))
                    }
                    _ => 0,
                };
                let laid_out_for = (magnifies.then_some(overscaled_zoom), pixel_ratio);
                if is_current(world, coords, laid_out_for) {
                    continue;
                }
                // The rest wait for a later frame, once tiles in flight have landed.
                if budget == 0 {
                    continue;
                }
                budget -= 1;

                self.request(coords, style, world, (overscaled_zoom, pixel_ratio))?;
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
        wanted: &HashSet<crate::coords::WorldTileCoords>,
    ) {
        for attempt in crate::sdf::provided::release_unwanted(world, wanted) {
            self.kernel.apc().cancel(attempt);
        }
    }

    fn request(
        &self,
        coords: crate::coords::WorldTileCoords,
        style: &crate::style::Style,
        world: &mut crate::tcs::world::World,
        (overscaled_zoom, pixel_ratio): (u8, f32),
    ) -> SystemResult {
        if world.tiles.spawn_mut(coords).is_none() {
            return Err(SystemError::InvalidTile { coords });
        }
        let attempt = tile_retry::next_attempt(world);
        self.kernel
            .apc()
            .call(
                Input::TrackedTileRequest {
                    coords,
                    style: style.clone(),
                    attempt,
                    overscaled_zoom,
                    pixel_ratio,
                },
                fetch_vector_apc::<E::OffscreenKernelEnvironment, T, _>,
            )
            .map_err(|source| SystemError::TileRequest {
                kind: "vector",
                coords,
                source,
            })?;
        super::content::begin(world, coords);
        if let Some(tile) = world
            .tiles
            .query_mut::<&mut VectorLayerBucketComponent>(coords)
        {
            tile.overscaled_zoom = overscaled_zoom;
        }
        tile_retry::started(world, coords, RequestKind::Vector, attempt);
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

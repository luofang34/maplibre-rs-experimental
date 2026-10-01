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
            ..
        }: &mut MapContext,
    ) -> SystemResult {
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
                if let Some(tile) = world.tiles.query::<&VectorLayerBucketComponent>(coords) {
                    let current = !magnifies || tile.overscaled_zoom == overscaled_zoom;
                    if current && !tile_retry::due(world, coords, RequestKind::Vector) {
                        continue;
                    }
                }
                // The rest wait for a later frame, once tiles in flight have landed.
                if budget == 0 {
                    break;
                }
                budget -= 1;

                self.request(coords, style, world, overscaled_zoom)?;
            }
        }
        Ok(())
    }
}

impl<E: Environment, T: VectorTransferables> RequestSystem<E, T> {
    fn request(
        &self,
        coords: crate::coords::WorldTileCoords,
        style: &crate::style::Style,
        world: &mut crate::tcs::world::World,
        overscaled_zoom: u8,
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
        tracing::debug!(%coords, "vector tile request accepted");
        Ok(())
    }
}

/// Whether tiles are laid out for the zoom they are magnified to: layout and filter
/// expressions are evaluated at that zoom, and labels along a line space themselves by it.
pub fn depends_on_overscaling(style: &crate::style::Style) -> bool {
    style.layers.iter().any(|layer| layer.paint.is_some())
}

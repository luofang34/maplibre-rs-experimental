//! Requests tiles which are currently in view

use std::{borrow::Cow, collections::HashSet, marker::PhantomData, rc::Rc};

use crate::{
    context::MapContext,
    coords::WorldTileCoords,
    environment::{Environment, OffscreenKernel},
    io::{
        apc::{AsyncProcedureCall, AsyncProcedureFuture, Context, Input, ProcedureError},
        tile_backpressure::request_budget,
        tile_sources::{missing_tile_fallback, source_layer_groups, source_min_zoom, TileKind},
    },
    kernel::Kernel,
    raster::{
        process_raster::{process_raster_tile, ProcessRasterContext, RasterTileRequest},
        transferables::{LayerRasterMissing, RasterTransferables},
        RasterLayersDataComponent,
    },
    render::{projection::raster_source_regions, view_state::ViewStatePadding},
    tcs::system::{System, SystemResult},
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
            ..
        }: &mut MapContext,
    ) -> SystemResult {
        // Missing ancestors and deferred tiles must progress while the camera is stationary.
        // Each raster source covers the view at its own tile size and rounding, as GL JS's
        // per-source tile managers do; the tiles of every source are requested together.
        let regions = raster_source_regions(style, view_state, world, ViewStatePadding::Loose)
            .map_err(|error| {
                tracing::error!(%error, "unable to select raster request tiles");
                crate::tcs::system::SystemError::Setup
            })?;
        let mut requested = HashSet::new();
        let mut budget = request_budget(world);
        let minzoom = source_min_zoom(style, TileKind::Raster).unwrap_or(0);
        // A tile the source answered 404 for is stood in for by its nearest ancestor,
        // as GL JS retains and loads parents for it.
        let wanted: Vec<WorldTileCoords> = regions
            .into_iter()
            .flat_map(|(_, tiles)| tiles)
            .flat_map(|coords| {
                let fallback = missing_tile_fallback(coords, minzoom, |coords| {
                    world
                        .tiles
                        .query::<&RasterLayersDataComponent>(coords)
                        .is_some_and(RasterLayersDataComponent::is_missing)
                });
                [Some(coords), fallback].into_iter().flatten()
            })
            .collect();
        for coords in wanted {
            if !requested.insert(coords) {
                continue;
            }

            // TODO: Make tessellation depend on style? So maybe we need to request even if it exists
            if world
                .tiles
                .query::<&RasterLayersDataComponent>(coords)
                .is_some()
            {
                continue;
            }
            // The rest wait for a later frame, once tiles in flight have landed.
            if budget == 0 {
                break;
            }
            budget -= 1;

            self.request(coords, style, world)?;
        }

        Ok(())
    }
}
pub fn fetch_raster_apc<K: OffscreenKernel, T: RasterTransferables, C: Context + Clone + Send>(
    input: Input,
    context: C,
    kernel: K,
) -> AsyncProcedureFuture {
    Box::pin(async move {
        let Input::TileRequest { coords, style } = input else {
            return Err(ProcedureError::IncompatibleInput);
        };

        let client = kernel.source_client();

        for group in source_layer_groups(&style, TileKind::Raster) {
            let context = context.clone();
            match client.fetch(&coords, &group.source).await {
                Ok(data) => {
                    let data = data.into_boxed_slice();

                    let mut process_context = ProcessRasterContext::<T, C>::new(context);

                    process_raster_tile(&data, RasterTileRequest { coords }, &mut process_context)
                        .map_err(|e| ProcedureError::Execution(Box::new(e)))?;
                }
                Err(error) => {
                    if error.is_not_found() {
                        tracing::debug!(
                            %coords,
                            source = ?group.source_name,
                            "no raster tile at the source; the layer is empty"
                        );
                    } else {
                        tracing::error!(
                            %coords,
                            source = ?group.source_name,
                            error = %error.describe(),
                            "raster tile fetch failed"
                        );
                    }

                    context
                        .send_back(<T as RasterTransferables>::LayerRasterMissing::build_from(
                            coords,
                        ))
                        .map_err(ProcedureError::Send)?;
                }
            }
        }

        Ok(())
    })
}

impl<E: Environment, T: RasterTransferables> RequestSystem<E, T> {
    fn request(
        &self,
        coords: crate::coords::WorldTileCoords,
        style: &crate::style::Style,
        world: &mut crate::tcs::world::World,
    ) -> SystemResult {
        let Some(mut tile) = world.tiles.spawn_mut(coords) else {
            return Err(crate::tcs::system::SystemError::Setup);
        };
        tile.insert(RasterLayersDataComponent::default());
        tracing::debug!(%coords, "tile request started");
        self.kernel.apc().call(Input::TileRequest { coords, style: style.clone() },
            fetch_raster_apc::<E::OffscreenKernelEnvironment, T, <E::AsyncProcedureCall as AsyncProcedureCall<E::OffscreenKernelEnvironment>>::Context>)
            .map_err(|error| {
                tracing::error!(%coords, ?error, "unable to schedule tile request");
                crate::tcs::system::SystemError::Setup
            })
    }
}

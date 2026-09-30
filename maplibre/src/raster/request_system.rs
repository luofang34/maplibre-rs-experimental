//! Requests tiles which are currently in view

use std::{borrow::Cow, collections::HashSet, marker::PhantomData, rc::Rc};

use crate::{
    context::MapContext,
    coords::WorldTileCoords,
    environment::{Environment, OffscreenKernel},
    io::{
        apc::{
            AsyncProcedureCall, AsyncProcedureFuture, AttemptContext, Context, Input,
            ProcedureError,
        },
        tile_backpressure::request_budget,
        tile_retry::{self, RequestDisposition, RequestKind, TileRequestOutcome},
        tile_sources::{missing_tile_fallback, source_layer_groups, TileKind},
    },
    kernel::Kernel,
    raster::{
        process_raster::{
            process_raster_tile, ProcessRasterContext, ProcessRasterError, RasterTileRequest,
        },
        transferables::{LayerRasterMissing, RasterTransferables},
        RasterLayersDataComponent, RasterSourceId,
    },
    render::{projection::raster_source_regions, view_state::ViewStatePadding},
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
            ..
        }: &mut MapContext,
    ) -> SystemResult {
        // Missing ancestors and deferred tiles must progress while the camera is stationary.
        // Each raster source covers the view at its own tile size and rounding, as GL JS's
        // per-source tile managers do; the tiles of every source are requested together.
        let regions = raster_source_regions(style, view_state, world, ViewStatePadding::Loose)?;
        let mut requested = HashSet::new();
        let mut budget = request_budget(world);
        let wanted = wanted_tiles(regions, style, world);
        let sources: Vec<_> = source_layer_groups(style, TileKind::Raster)
            .into_iter()
            .map(|group| RasterSourceId::new(group.source_name))
            .collect();
        for coords in wanted {
            if !requested.insert(coords) {
                continue;
            }

            let retry_due = tile_retry::due(world, coords, RequestKind::Raster);
            if tile_retry::waiting(world, coords, RequestKind::Raster) && !retry_due {
                continue;
            }
            if !retry_due
                && world
                    .tiles
                    .query::<&RasterLayersDataComponent>(coords)
                    .is_some_and(|component| {
                        sources
                            .iter()
                            .all(|source| component.has_source_result(source))
                    })
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
        let (coords, style, attempt) = input.into_tile_request();
        let context = AttemptContext::new(context, attempt);

        let client = kernel.source_client();
        let mut retry = false;

        for group in source_layer_groups(&style, TileKind::Raster) {
            let context = context.clone();
            match client.fetch(&coords, &group.source).await {
                Ok(data) => {
                    let mut process_context = ProcessRasterContext::<T, _>::new(context.clone());
                    match process_raster_tile(
                        &data,
                        RasterTileRequest {
                            coords,
                            source: RasterSourceId::new(group.source_name.clone()),
                        },
                        &mut process_context,
                    ) {
                        Ok(()) => {}
                        Err(ProcessRasterError::Send(source)) => {
                            return Err(ProcedureError::Send(source))
                        }
                        Err(ProcessRasterError::Decoding { source }) => {
                            tracing::warn!(%coords, source = ?group.source_name, error = %source, "invalid raster tile");
                            context
                                .send_back(T::LayerRasterMissing::build_from(
                                    coords,
                                    RasterSourceId::new(group.source_name.clone()),
                                ))
                                .map_err(ProcedureError::Send)?;
                        }
                    }
                }
                Err(error) => {
                    retry |= error.is_retryable();
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
                            RasterSourceId::new(group.source_name.clone()),
                        ))
                        .map_err(ProcedureError::Send)?;
                }
            }
        }

        context
            .send_back(TileRequestOutcome {
                coords,
                kind: RequestKind::Raster,
                attempt,
                disposition: if retry {
                    RequestDisposition::Retry
                } else {
                    RequestDisposition::Complete
                },
            })
            .map_err(ProcedureError::Send)
    })
}

impl<E: Environment, T: RasterTransferables> RequestSystem<E, T> {
    fn request(
        &self,
        coords: crate::coords::WorldTileCoords,
        style: &crate::style::Style,
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
        self.kernel
            .apc()
            .call(
                Input::TrackedTileRequest {
                    attempt,
                    coords,
                    style: style.clone(),
                },
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
        tile_retry::started(world, coords, RequestKind::Raster, attempt);
        tracing::debug!(%coords, "raster tile request accepted");
        Ok(())
    }
}

fn wanted_tiles(
    regions: Vec<(RasterSourceId, Vec<WorldTileCoords>)>,
    style: &crate::style::Style,
    world: &crate::tcs::world::World,
) -> Vec<WorldTileCoords> {
    let mut wanted = Vec::new();
    for (source, tiles) in regions {
        let minzoom = match source.name().and_then(|name| style.sources.get(name)) {
            Some(crate::style::source::Source::Raster(source)) => source.minzoom,
            Some(crate::style::source::Source::RasterDem(source)) => source.minzoom,
            _ => None,
        }
        .unwrap_or(0);
        let bounds = match source.name().and_then(|name| style.sources.get(name)) {
            Some(crate::style::source::Source::Raster(source)) => source.bounds,
            _ => None,
        };
        for coords in tiles {
            // Tiles outside the source's declared bounds are never requested.
            if bounds.is_some_and(|bounds| !crate::io::tile_sources::tile_in_bounds(coords, bounds))
            {
                continue;
            }
            wanted.push(coords);
            if let Some(parent) = missing_tile_fallback(coords, minzoom, |coords| {
                world
                    .tiles
                    .query::<&RasterLayersDataComponent>(coords)
                    .is_some_and(|component| component.source_is_missing(&source))
            }) {
                wanted.push(parent);
            }
        }
    }
    wanted
}

#[cfg(test)]
mod tests;

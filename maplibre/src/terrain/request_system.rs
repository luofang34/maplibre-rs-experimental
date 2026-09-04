//! Requests the DEM tiles covering the tiles in view.

use std::{borrow::Cow, collections::HashSet, marker::PhantomData, rc::Rc};

use crate::{
    context::MapContext,
    coords::WorldTileCoords,
    environment::{Environment, OffscreenKernel},
    io::apc::{AsyncProcedureCall, AsyncProcedureFuture, Context, Input, ProcedureError},
    kernel::Kernel,
    render::{
        projection::view_region_for_projection, tile_view_pattern::DEFAULT_TILE_SIZE,
        view_state::ViewStatePadding,
    },
    tcs::system::{System, SystemError, SystemResult},
    terrain::{
        source::dem_source,
        transferables::{DemTransferables, LayerDem, LayerDemMissing},
        DemTileComponent,
    },
};

/// Zoom levels between a draped tile and the DEM tile it samples, as in GL JS `deltaZoom`.
const DELTA_ZOOM: u8 = 1;

/// Returns the DEM tile a view tile samples: one zoom level up, clamped to the source range.
pub fn dem_tile_coords(
    coords: WorldTileCoords,
    minzoom: u8,
    maxzoom: u8,
) -> Option<WorldTileCoords> {
    let zoom = u8::from(coords.z).saturating_sub(DELTA_ZOOM).min(maxzoom);
    if zoom < minzoom {
        return None;
    }
    let mut current = coords;
    while u8::from(current.z) > zoom {
        current = current.get_parent()?;
    }
    Some(current)
}

pub struct RequestSystem<E: Environment, T: DemTransferables> {
    kernel: Rc<Kernel<E>>,
    phantom_t: PhantomData<T>,
}

impl<E: Environment, T: DemTransferables> RequestSystem<E, T> {
    pub fn new(kernel: &Rc<Kernel<E>>) -> Self {
        Self {
            kernel: kernel.clone(),
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
        let Some(dem) = dem_source(style) else {
            return Ok(());
        };
        let Some(view_region) = view_region_for_projection(
            style,
            view_state,
            view_state.zoom().zoom_level(DEFAULT_TILE_SIZE),
            ViewStatePadding::Loose,
        )
        .map_err(|error| {
            tracing::error!(%error, "unable to select DEM request tiles");
            SystemError::Setup
        })?
        else {
            return Ok(());
        };

        let mut requested = HashSet::new();
        for coords in view_region.iter() {
            let Some(coords) = dem_tile_coords(coords, dem.minzoom, dem.maxzoom) else {
                continue;
            };
            if coords.build_quad_key().is_none() || !requested.insert(coords) {
                continue;
            }
            if world.tiles.query::<&DemTileComponent>(coords).is_some() {
                continue;
            }
            let Some(mut tile) = world.tiles.spawn_mut(coords) else {
                continue;
            };
            tile.insert(DemTileComponent::Pending);
            tracing::debug!(%coords, "DEM tile request started");

            if let Err(error) =
                self.kernel.apc().call(
                    Input::TileRequest {
                        coords,
                        style: style.clone(),
                    },
                    fetch_dem_apc::<
                        E::OffscreenKernelEnvironment,
                        T,
                        <E::AsyncProcedureCall as AsyncProcedureCall<
                            E::OffscreenKernelEnvironment,
                        >>::Context,
                    >,
                )
            {
                tracing::error!(%coords, ?error, "unable to schedule DEM tile request");
            }
        }

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
        let Input::TileRequest { coords, style } = input else {
            return Err(ProcedureError::IncompatibleInput);
        };
        let Some(dem) = dem_source(&style) else {
            return Ok(());
        };
        let image = match kernel.source_client().fetch(&coords, &dem.source).await {
            Ok(data) => match image::load_from_memory(&data) {
                Ok(image) => Some(image.to_rgba8()),
                Err(error) => {
                    tracing::warn!(%coords, source = %dem.name, %error, "DEM tile is not an image");
                    None
                }
            },
            Err(error) => {
                tracing::error!(%coords, source = %dem.name, %error, "DEM tile fetch failed");
                None
            }
        };
        match image {
            Some(image) => context
                .send_back(T::LayerDem::build_from(coords, image))
                .map_err(ProcedureError::Send),
            None => context
                .send_back(T::LayerDemMissing::build_from(coords))
                .map_err(ProcedureError::Send),
        }
    })
}

#[cfg(test)]
mod tests;

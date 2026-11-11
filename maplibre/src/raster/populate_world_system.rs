use std::{borrow::Cow, marker::PhantomData, rc::Rc, sync::Arc};

use image::RgbaImage;

use crate::{
    context::MapContext,
    environment::Environment,
    io::{
        apc::{apply_worker_messages, AsyncProcedureCall, Message, MessageError},
        tile_retry::{self, RequestKind, TileRequestOutcome},
    },
    kernel::Kernel,
    raster::{
        resource::RasterResources,
        transferables::{LayerRaster, LayerRasterMissing, RasterTransferables},
        AvailableRasterLayerData, MissingRasterLayerData, RasterLayerData,
        RasterLayersDataComponent,
    },
    render::eventually::Eventually,
    style::Style,
    tcs::{
        system::{System, SystemResult},
        world::World,
    },
    terrain::{
        dem_store::{
            decode_image_to_dem, dem_unpack_for_source, ingest_source_dem, is_terrain_source,
        },
        LoadedDem,
    },
};

pub struct PopulateWorldSystem<E: Environment, T> {
    kernel: Rc<Kernel<E>>,
    phantom_t: PhantomData<T>,
}

impl<E: Environment, T> PopulateWorldSystem<E, T> {
    pub fn new(kernel: &Rc<Kernel<E>>) -> Self {
        Self {
            kernel: kernel.clone(),
            phantom_t: Default::default(),
        }
    }
}

impl<E: Environment, T: RasterTransferables> System for PopulateWorldSystem<E, T> {
    fn name(&self) -> Cow<'static, str> {
        "populate_world_system".into()
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
        let circumference = view_state.body().circumference_meters();
        let messages = self.kernel.apc().receive(|message| {
            message.has_tag(RequestKind::Raster.message_tag())
                || message.has_tag(T::LayerRaster::message_tag())
                || message.has_tag(T::LayerRasterMissing::message_tag())
        });
        apply_worker_messages(messages, |message| {
            apply_raster_message_with_style::<T>(world, Some(style), circumference, message)
        })?;

        Ok(())
    }
}

/// Records a worker's raster result on its tile without consulting a style (`raster` imagery default).
#[cfg(test)]
pub(crate) fn apply_raster_message<T: RasterTransferables>(
    world: &mut World,
    message: Message,
) -> Result<(), MessageError> {
    apply_raster_message_with_style::<T>(
        world,
        None,
        crate::projection::body::Body::EARTH.circumference_meters(),
        message,
    )
}

/// Records a worker's raster or `raster-dem` result on its tile. `raster-dem` sources are stored
/// once as 2-texel bordered [`LoadedDem`] tiles in [`DemLayersComponent`](crate::terrain::DemLayersComponent)
/// and mirrored to 3D terrain when shared.
pub(crate) fn apply_raster_message_with_style<T: RasterTransferables>(
    world: &mut World,
    style: Option<&Style>,
    circumference_meters: f64,
    message: Message,
) -> Result<(), MessageError> {
    if message.has_tag(RequestKind::Raster.message_tag()) {
        tile_retry::completed(world, *message.into_transferable::<TileRequestOutcome>()?);
        return Ok(());
    }
    let attempt = message.attempt();
    let (coords, layer) = if message.has_tag(T::LayerRaster::message_tag()) {
        let message = message.into_transferable::<T::LayerRaster>()?;
        let coords = message.coords();
        if !tile_retry::accepts(world, coords, RequestKind::Raster, attempt)
            || world
                .tiles
                .query::<&RasterLayersDataComponent>(coords)
                .is_none()
        {
            return Ok(());
        }
        let payload = message.into_dem_or_layer();
        let source = match &payload {
            Ok(shared) => shared.source.clone(),
            Err(layer) => layer.source.clone(),
        };
        if let Some((style, unpack)) =
            style.and_then(|s| dem_unpack_for_source(s, &source).map(|u| (s, u)))
        {
            let loaded = match payload {
                Ok(shared)
                    if shared.tile.unpack() == unpack
                        && (shared.circumference_meters - circumference_meters).abs() <= 1e-3 =>
                {
                    Some(LoadedDem::from_shared(shared.tile))
                }
                Ok(shared) => {
                    decode_image_to_dem(coords, &shared.fallback, unpack, circumference_meters)
                }
                Err(layer) => {
                    decode_image_to_dem(coords, &layer.image, unpack, circumference_meters)
                }
            };
            match loaded {
                Some(dem) => {
                    ingest_source_dem(
                        &mut world.tiles,
                        &source,
                        coords,
                        dem,
                        is_terrain_source(style, &source),
                    );
                    (
                        coords,
                        RasterLayerData::Available(AvailableRasterLayerData {
                            coords,
                            source,
                            image: RgbaImage::new(0, 0),
                        }),
                    )
                }
                None => (
                    coords,
                    RasterLayerData::Missing(MissingRasterLayerData { coords, source }),
                ),
            }
        } else {
            let available = match payload {
                Err(layer) => layer,
                Ok(shared) => AvailableRasterLayerData {
                    coords: shared.coords,
                    source: shared.source,
                    image: Arc::try_unwrap(shared.fallback).unwrap_or_else(|arc| (*arc).clone()),
                },
            };
            (coords, RasterLayerData::Available(available))
        }
    } else if message.has_tag(T::LayerRasterMissing::message_tag()) {
        let message = message.into_transferable::<T::LayerRasterMissing>()?;
        let coords = message.coords();
        if !tile_retry::accepts(world, coords, RequestKind::Raster, attempt) {
            return Ok(());
        }
        (coords, RasterLayerData::Missing(message.to_layer()))
    } else {
        return Ok(());
    };

    let source = layer.source().clone();
    let Some(component) = world
        .tiles
        .query_mut::<&mut RasterLayersDataComponent>(coords)
    else {
        return Ok(());
    };
    if component.record(layer) {
        if let Some(Eventually::Initialized(raster)) =
            world.resources.get_mut::<Eventually<RasterResources>>()
        {
            raster.tile_data_changed(&source, coords);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;

use std::{borrow::Cow, marker::PhantomData, rc::Rc};

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
        RasterLayerData, RasterLayersDataComponent,
    },
    render::eventually::Eventually,
    tcs::{
        system::{System, SystemResult},
        world::World,
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

    fn run(&mut self, MapContext { world, .. }: &mut MapContext) -> SystemResult {
        let messages = self.kernel.apc().receive(|message| {
            message.has_tag(RequestKind::Raster.message_tag())
                || message.has_tag(T::LayerRaster::message_tag())
                || message.has_tag(T::LayerRasterMissing::message_tag())
        });
        apply_worker_messages(messages, |message| {
            apply_raster_message::<T>(world, message)
        })?;

        Ok(())
    }
}

/// Records a worker's raster result on its tile. A fetched image becomes an available layer and
/// a failed fetch a missing one. Final request results schedule retries independently of
/// the pixels retained for drawing.
pub(crate) fn apply_raster_message<T: RasterTransferables>(
    world: &mut World,
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
        if !tile_retry::accepts(world, coords, RequestKind::Raster, attempt) {
            return Ok(());
        }
        (coords, RasterLayerData::Available(message.to_layer()))
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
            raster.remove_source_texture(&source, coords);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;

use std::{borrow::Cow, marker::PhantomData, rc::Rc};

use crate::{
    context::MapContext,
    environment::Environment,
    io::apc::{AsyncProcedureCall, Message},
    kernel::Kernel,
    raster::{
        transferables::{LayerRaster, LayerRasterMissing, RasterTransferables},
        RasterLayerData, RasterLayersDataComponent,
    },
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
        for message in self.kernel.apc().receive(|message| {
            message.has_tag(T::LayerRaster::message_tag())
                || message.has_tag(T::LayerRasterMissing::message_tag())
        }) {
            apply_raster_message::<T>(world, message);
        }

        Ok(())
    }
}

/// Records a worker's raster result on its tile. A fetched image becomes an available layer and
/// a failed fetch a missing one, so the tile counts as done either way instead of being
/// requested again forever.
pub(crate) fn apply_raster_message<T: RasterTransferables>(world: &mut World, message: Message) {
    let (coords, layer) = if message.has_tag(T::LayerRaster::message_tag()) {
        let message = message.into_transferable::<T::LayerRaster>();
        (
            message.coords(),
            RasterLayerData::Available(message.to_layer()),
        )
    } else if message.has_tag(T::LayerRasterMissing::message_tag()) {
        let message = message.into_transferable::<T::LayerRasterMissing>();
        (
            message.coords(),
            RasterLayerData::Missing(message.to_layer()),
        )
    } else {
        return;
    };

    let Some(component) = world
        .tiles
        .query_mut::<&mut RasterLayersDataComponent>(coords)
    else {
        return;
    };
    component.layers.push(layer);
}

#[cfg(test)]
mod tests;

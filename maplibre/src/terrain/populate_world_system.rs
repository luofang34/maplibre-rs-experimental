//! Moves decoded DEM tiles from worker messages into the world.

use std::{borrow::Cow, marker::PhantomData, rc::Rc};

use crate::{
    context::MapContext,
    environment::Environment,
    io::apc::{AsyncProcedureCall, Message},
    kernel::Kernel,
    tcs::system::{System, SystemResult},
    terrain::{
        dem::DemTile,
        source::dem_source,
        transferables::{DemTransferables, LayerDem, LayerDemMissing},
        DemTileComponent,
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
            phantom_t: PhantomData,
        }
    }
}

impl<E: Environment, T: DemTransferables> System for PopulateWorldSystem<E, T> {
    fn name(&self) -> Cow<'static, str> {
        "dem_populate_world".into()
    }

    fn run(&mut self, MapContext { style, world, .. }: &mut MapContext) -> SystemResult {
        let unpack = dem_source(style).map(|dem| dem.unpack);
        for message in self.kernel.apc().receive(|message| {
            message.has_tag(T::LayerDem::message_tag())
                || message.has_tag(T::LayerDemMissing::message_tag())
        }) {
            let message: Message = message;
            let (coords, state) = if message.has_tag(T::LayerDem::message_tag()) {
                let message = message.into_transferable::<T::LayerDem>();
                let coords = message.coords();
                let state =
                    match unpack.map(|unpack| DemTile::from_image(&message.into_image(), unpack)) {
                        Some(Ok(tile)) => DemTileComponent::Loaded(tile),
                        Some(Err(error)) => {
                            tracing::warn!(%coords, %error, "DEM tile image is unusable");
                            DemTileComponent::Missing
                        }
                        None => DemTileComponent::Missing,
                    };
                (coords, state)
            } else {
                let message = message.into_transferable::<T::LayerDemMissing>();
                (message.coords(), DemTileComponent::Missing)
            };
            if let Some(component) = world.tiles.query_mut::<&mut DemTileComponent>(coords) {
                *component = state;
            }
        }
        Ok(())
    }
}

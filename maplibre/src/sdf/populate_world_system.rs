use std::{borrow::Cow, marker::PhantomData, rc::Rc};

use crate::{
    context::MapContext,
    environment::Environment,
    io::{
        apc::{apply_worker_messages, AsyncProcedureCall},
        tile_retry::{self, RequestKind},
    },
    kernel::Kernel,
    tcs::system::{System, SystemResult},
    vector::transferables::*,
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

impl<E: Environment, T: VectorTransferables> System for PopulateWorldSystem<E, T> {
    fn name(&self) -> Cow<'static, str> {
        "sdf_populate_world_system".into()
    }

    fn run(&mut self, MapContext { world, style, .. }: &mut MapContext) -> SystemResult {
        let messages = self
            .kernel
            .apc()
            .receive(|message| message.has_tag(T::SymbolLayerTessellated::message_tag()));
        apply_worker_messages(messages, |message| {
            if message.has_tag(T::SymbolLayerTessellated::message_tag()) {
                let attempt = message.attempt();
                let message = message.into_transferable::<T::SymbolLayerTessellated>()?;
                let coords = message.coords();
                let bucket = message.to_bucket();
                if tile_retry::accepts(world, coords, RequestKind::Vector, attempt)
                    && style
                        .layers
                        .iter()
                        .any(|layer| layer.id == bucket.style_layer_id)
                {
                    crate::vector::content::accept_symbols(world, bucket);
                }
            }
            Ok(())
        })?;

        Ok(())
    }
}

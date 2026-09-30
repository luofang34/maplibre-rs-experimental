use std::{borrow::Cow, marker::PhantomData, rc::Rc};

use crate::{
    context::MapContext,
    environment::Environment,
    io::{
        apc::{apply_worker_messages, AsyncProcedureCall},
        tile_retry::{self, RequestKind, TileRequestOutcome},
    },
    kernel::Kernel,
    tcs::system::{System, SystemResult},
    vector::{transferables::*, VectorLayerBucket, VectorLayerBucketComponent},
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
        "populate_world_system".into()
    }

    fn run(&mut self, MapContext { world, style, .. }: &mut MapContext) -> SystemResult {
        let messages = self.kernel.apc().receive(|message| {
            message.has_tag(RequestKind::Vector.message_tag())
                || message.has_tag(T::TileTessellated::message_tag())
                || message.has_tag(T::LayerMissing::message_tag())
                || message.has_tag(T::LayerTessellated::message_tag())
                || message.has_tag(T::LayerIndexed::message_tag())
        });
        apply_worker_messages(messages, |message| {
            let attempt = message.attempt();
            if message.has_tag(RequestKind::Vector.message_tag()) {
                tile_retry::completed(world, *message.into_transferable::<TileRequestOutcome>()?);
            } else if message.has_tag(T::TileTessellated::message_tag()) {
                let message = message.into_transferable::<T::TileTessellated>()?;
                if tile_retry::accepts(world, message.coords(), RequestKind::Vector, attempt) {
                    finish_tile(world, &*message);
                }
            } else if message.has_tag(T::LayerMissing::message_tag()) {
                let message = message.into_transferable::<T::LayerMissing>()?;
                if !tile_retry::accepts(world, message.coords(), RequestKind::Vector, attempt) {
                    return Ok(());
                }
                let Some(component) = world
                    .tiles
                    .query_mut::<&mut VectorLayerBucketComponent>(message.coords())
                else {
                    return Ok(());
                };

                if !component.layers.iter().any(|layer| match layer {
                    VectorLayerBucket::AvailableLayer(layer) => {
                        layer.source_layer == message.layer_name()
                    }
                    VectorLayerBucket::Missing(layer) => layer.source_layer == message.layer_name(),
                }) {
                    component
                        .layers
                        .push(VectorLayerBucket::Missing(message.to_bucket()));
                }
            } else if message.has_tag(T::LayerTessellated::message_tag()) {
                let message = message.into_transferable::<T::LayerTessellated>()?;
                // A layer removed while its tile was in flight must not come back with the reply.
                let coords = message.coords();
                let bucket = message.to_bucket();
                if tile_retry::accepts(world, coords, RequestKind::Vector, attempt)
                    && style
                        .layers
                        .iter()
                        .any(|layer| layer.id == bucket.style_layer_id)
                {
                    super::content::accept_vector(world, bucket);
                }
            } else if message.has_tag(T::LayerIndexed::message_tag()) {
                let message = message.into_transferable::<T::LayerIndexed>()?;
                if !tile_retry::accepts(world, message.coords(), RequestKind::Vector, attempt)
                    || world
                        .tiles
                        .query::<&VectorLayerBucketComponent>(message.coords())
                        .is_none()
                {
                    return Ok(());
                }
                world
                    .tiles
                    .geometry_index
                    .index_tile(&message.coords(), message.to_tile_index());
            }
            Ok(())
        })?;

        Ok(())
    }
}

fn finish_tile<T: TileTessellated>(world: &mut crate::tcs::world::World, message: &T) {
    let retained = world
        .tiles
        .query::<&super::content::LayerReplacements>(message.coords())
        .map(|pending| (pending.keep_vector, pending.keep_symbols))
        .unwrap_or_default();
    if let Some(component) = world
        .tiles
        .query_mut::<&mut VectorLayerBucketComponent>(message.coords())
    {
        let usable = component.done && !component.failed;
        component.done = true;
        component.failed = message.failed() && !retained.0 && !usable;
    }
    if let Some(component) = world
        .tiles
        .query_mut::<&mut crate::sdf::SymbolLayersDataComponent>(message.coords())
    {
        component.pending_assets = message.pending_symbols() && !retained.1;
    }
}

#[cfg(test)]
mod tests;

use std::{borrow::Cow, marker::PhantomData, rc::Rc};

use crate::{
    context::MapContext,
    environment::Environment,
    io::{
        apc::{apply_worker_messages, AsyncProcedureCall, Message, MessageError},
        tile_retry::{self, RequestKind, TileRequestOutcome},
    },
    kernel::Kernel,
    style::Style,
    tcs::{
        system::{System, SystemResult},
        world::World,
    },
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
        apply_vector_worker_batch::<T>(world, style, messages)?;

        Ok(())
    }
}

/// Applies a batch of vector worker messages to `world`, synchronizing runtime `feature-state`
/// and deduplicating decoded `Arc<FeatureProperties>` maps across layers and the query index.
pub fn apply_vector_worker_batch<T: VectorTransferables>(
    world: &mut World,
    style: &Style,
    messages: impl IntoIterator<Item = Message>,
) -> Result<(), MessageError> {
    let mut memo = crate::style::feature_state::FeatureStateArcMemo::default();
    let mut interner = super::feature_paint::intern::TilePropertyInterner::default();
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
            let mut message = message.into_transferable::<T::LayerTessellated>()?;
            // A layer removed while its tile was in flight must not come back with the reply.
            let coords = message.coords();
            let mut feature_paint = message.take_feature_paint();
            let mut bucket = message.to_bucket();
            if tile_retry::accepts(world, coords, RequestKind::Vector, attempt)
                && style
                    .layers
                    .iter()
                    .any(|layer| layer.id == bucket.style_layer_id)
            {
                let overscaled_zoom = world
                    .tiles
                    .query::<&VectorLayerBucketComponent>(coords)
                    .map_or(0, |b| b.overscaled_zoom);
                let retessellate = super::arrival_sync::sync_arriving_vector_bucket(
                    style,
                    overscaled_zoom,
                    &mut bucket,
                    &mut feature_paint,
                    &mut memo,
                );
                if let Some(entries) = feature_paint.as_mut() {
                    interner.intern_entries(&world.tiles, coords, entries);
                }
                super::content::accept_vector_with_paint(world, bucket, feature_paint);
                if retessellate {
                    tile_retry::refresh_tiles(world, RequestKind::Vector, &[coords]);
                }
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
            let coords = message.coords();
            let source = message.source();
            let mut index = message.to_tile_index();
            super::arrival_sync::sync_arriving_tile_index(
                style,
                source.as_deref(),
                &mut index,
                &mut memo,
            );
            interner.intern_tile_index(&world.tiles, coords, &mut index);
            world
                .tiles
                .geometry_index
                .index_tile(&coords, source, index);
        }
        Ok(())
    })
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
    // A request that could not read its data says nothing about which layers are empty.
    if !message.failed() {
        super::content::drop_undelivered(world, message.coords());
    }
}

#[cfg(test)]
mod tests;

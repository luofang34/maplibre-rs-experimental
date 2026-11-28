use std::{borrow::Cow, marker::PhantomData, rc::Rc};

use crate::{
    context::MapContext,
    environment::Environment,
    io::{
        apc::{apply_worker_messages, AsyncProcedureCall, Message, MessageError},
        tile_retry::{self, RequestKind, TileRequestOutcome},
    },
    kernel::Kernel,
    schedule::system::{System, SystemResult},
    style::Style,
    tcs::world::World,
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
        let mut messages = self
            .kernel
            .apc()
            .receive(|message| {
                message.has_tag(RequestKind::Vector.message_tag())
                    || message.has_tag(T::TileTessellated::message_tag())
                    || message.has_tag(T::LayerMissing::message_tag())
                    || message.has_tag(T::LayerTessellated::message_tag())
                    || message.has_tag(T::LayerIndexed::message_tag())
            })
            .peekable();
        if messages.peek().is_some() {
            apply_vector_worker_batch::<T>(world, style, messages)?;
        }

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
    let mut interner = crate::vector::content::TilePropertyInterner::default();
    let layer_by_id = style.cached_layer_index_by_id();
    let mut accepted_any = false;
    apply_worker_messages(messages, |message| {
        let attempt = message.attempt();
        if message.has_tag(RequestKind::Vector.message_tag()) {
            accepted_any |= apply_request_outcome(world, message)?;
        } else if message.has_tag(T::TileTessellated::message_tag()) {
            let message = message.into_transferable::<T::TileTessellated>()?;
            if tile_retry::accepts(world, message.coords(), RequestKind::Vector, attempt) {
                finish_tile(world, &*message);
                accepted_any = true;
            }
        } else if message.has_tag(T::LayerMissing::message_tag()) {
            accepted_any |= apply_layer_missing::<T>(world, message, attempt)?;
        } else if message.has_tag(T::LayerTessellated::message_tag()) {
            accepted_any |= apply_layer_tessellated::<T>(
                world,
                style,
                &layer_by_id,
                message,
                attempt,
                &mut memo,
                &mut interner,
            )?;
        } else if message.has_tag(T::LayerIndexed::message_tag()) {
            accepted_any |=
                apply_layer_indexed::<T>(world, style, message, attempt, &mut memo, &mut interner)?;
        }
        Ok(())
    })?;
    if accepted_any {
        crate::vector::bump_arrival_epoch(world);
    }
    Ok(())
}

fn apply_request_outcome(world: &mut World, message: Message) -> Result<bool, MessageError> {
    #[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
    let mut message = message;
    #[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
    let shared_permit = message.take_shared_permit();
    let outcome = *message.into_transferable::<TileRequestOutcome>()?;
    #[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
    let coords = outcome.coords;
    tile_retry::completed(world, outcome);
    if let Some(remap) = world
        .resources
        .get_mut::<shared_keys::SharedGeometryKeyRemap>()
    {
        remap.finish(outcome.coords, outcome.attempt);
    }
    #[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
    if let Some(permit) = shared_permit {
        world
            .resources
            .get_or_init_mut::<super::request_system::PendingUploadPermits>()
            .push(coords, permit);
    }
    Ok(true)
}

fn apply_layer_missing<T: VectorTransferables>(
    world: &mut World,
    message: Message,
    attempt: Option<u64>,
) -> Result<bool, MessageError> {
    let message = message.into_transferable::<T::LayerMissing>()?;
    if !tile_retry::accepts(world, message.coords(), RequestKind::Vector, attempt) {
        return Ok(false);
    }
    let Some(component) = world
        .tiles
        .query_mut::<&mut VectorLayerBucketComponent>(message.coords())
    else {
        return Ok(false);
    };
    if !component.layers.iter().any(|layer| match layer {
        VectorLayerBucket::AvailableLayer(layer) => layer.source_layer == message.layer_name(),
        VectorLayerBucket::Missing(layer) => layer.source_layer == message.layer_name(),
    }) {
        component.uploaded_stamp = None;
        component
            .layers
            .push(VectorLayerBucket::Missing(message.to_bucket()));
        crate::vector::bump_upload_stamp_epoch(world);
    }
    Ok(true)
}

fn apply_layer_tessellated<T: VectorTransferables>(
    world: &mut World,
    style: &Style,
    layer_by_id: &rustc_hash::FxHashMap<String, usize>,
    message: Message,
    attempt: Option<u64>,
    memo: &mut crate::style::feature_state::FeatureStateArcMemo,
    interner: &mut crate::vector::content::TilePropertyInterner,
) -> Result<bool, MessageError> {
    let mut message = message.into_transferable::<T::LayerTessellated>()?;
    // A layer removed while its tile was in flight must not come back with the reply.
    let coords = message.coords();
    let mut feature_paint = message.take_feature_paint();
    let mut bucket = message.to_bucket();
    if let Some(style_layer) = tile_retry::accepts(world, coords, RequestKind::Vector, attempt)
        .then(|| layer_by_id.get(bucket.style_layer_id.as_str()))
        .flatten()
        .and_then(|&idx| style.layers.get(idx))
    {
        let overscaled_zoom = world
            .tiles
            .query::<&VectorLayerBucketComponent>(coords)
            .map_or(0, |b| b.overscaled_zoom);
        let retessellate = super::arrival_sync::sync_arriving_vector_bucket_for_layer(
            style,
            style_layer,
            overscaled_zoom,
            &mut bucket,
            &mut feature_paint,
            memo,
        );
        if let Some(entries) = feature_paint.as_mut() {
            interner.intern_entries(&world.tiles, coords, entries);
        }
        if let Some(raw_key) = bucket.buffer.shared_geometry_key() {
            let host_key = world
                .resources
                .get_or_init_mut::<shared_keys::SharedGeometryKeyRemap>()
                .remap(coords, attempt, raw_key);
            bucket.buffer.set_shared_geometry_key(Some(host_key));
        }
        crate::vector::content::accept_vector_with_paint(world, bucket, feature_paint);
        if retessellate {
            tile_retry::refresh_tiles(world, RequestKind::Vector, &[coords]);
        }
        return Ok(true);
    }
    Ok(false)
}

fn apply_layer_indexed<T: VectorTransferables>(
    world: &mut World,
    style: &Style,
    message: Message,
    attempt: Option<u64>,
    memo: &mut crate::style::feature_state::FeatureStateArcMemo,
    interner: &mut crate::vector::content::TilePropertyInterner,
) -> Result<bool, MessageError> {
    let message = message.into_transferable::<T::LayerIndexed>()?;
    if !tile_retry::accepts(world, message.coords(), RequestKind::Vector, attempt)
        || world
            .tiles
            .query::<&VectorLayerBucketComponent>(message.coords())
            .is_none()
    {
        return Ok(false);
    }
    let coords = message.coords();
    let source = message.source();
    let mut index = message.to_tile_index();
    super::arrival_sync::sync_arriving_tile_index(style, source.as_deref(), &mut index, memo);
    interner.intern_tile_index(&world.tiles, coords, &mut index);
    world.tiles.as_mut().index_tile(&coords, source, index);
    Ok(true)
}

fn finish_tile<T: TileTessellated>(world: &mut crate::tcs::world::World, message: &T) {
    let keep_vector = world
        .tiles
        .query::<&crate::vector::content::LayerReplacements>(message.coords())
        .is_some_and(|pending| pending.keep_vector);
    let keep_symbols = world
        .tiles
        .query::<&crate::sdf::SymbolLayerReplacements>(message.coords())
        .is_some_and(|pending| pending.keep_symbols);
    let mut touched_vector = false;
    if let Some(component) = world
        .tiles
        .query_mut::<&mut VectorLayerBucketComponent>(message.coords())
    {
        let usable = component.done && !component.failed;
        component.done = true;
        component.failed = message.failed() && !keep_vector && !usable;
        component.uploaded_stamp = None;
        touched_vector = true;
    }
    if touched_vector {
        crate::vector::bump_upload_stamp_epoch(world);
    }
    if let Some(component) = world
        .tiles
        .query_mut::<&mut crate::sdf::SymbolLayersDataComponent>(message.coords())
    {
        component.pending_assets = message.pending_symbols() && !keep_symbols;
        component.uploaded_stamp = None;
        component.atlas_zoom_q32 = None;
    }
    // A request that could not read its data says nothing about which layers are empty.
    if !message.failed() {
        crate::vector::content::drop_undelivered(world, message.coords());
    }
}

mod shared_keys;

#[cfg(test)]
mod tests;

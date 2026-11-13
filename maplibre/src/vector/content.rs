//! Successful layer replacements wait for their GPU data before replacing displayed content.

use super::{
    feature_paint::{BucketFeatureEntry, TileFeaturePaintComponent},
    AvailableVectorLayerBucket, VectorBufferPool, VectorLayerBucket, VectorLayerBucketComponent,
};
use crate::{
    coords::WorldTileCoords,
    render::eventually::Eventually,
    sdf::{SymbolBufferPool, SymbolLayerData, SymbolLayersDataComponent},
    tcs::{
        tiles::{TileComponent, Tiles},
        world::World,
    },
};

#[derive(Default)]
pub(crate) struct LayerReplacements {
    pub vector: Vec<AvailableVectorLayerBucket>,
    pub symbols: Vec<SymbolLayerData>,
    pub keep_vector: bool,
    pub keep_symbols: bool,
    pub revision: u64,
    /// Non-symbol style layers the request in flight was asked to build; its result is the
    /// tile's whole content for them, so those it leaves out are dropped when it finishes.
    pub requested: Vec<String>,
    /// Which of [`requested`](Self::requested) the request has delivered so far.
    pub delivered: Vec<String>,
}
impl TileComponent for LayerReplacements {}

#[derive(Default)]
struct ContentClock(u64);

pub(crate) fn ensure(tiles: &mut Tiles, coords: WorldTileCoords) {
    if !tiles.exists(coords) {
        return;
    }
    if tiles.query::<&LayerReplacements>(coords).is_none() {
        if let Some(mut tile) = tiles.spawn_mut(coords) {
            tile.insert(LayerReplacements::default());
        }
    }
    if tiles.query::<&TileFeaturePaintComponent>(coords).is_none() {
        if let Some(mut tile) = tiles.spawn_mut(coords) {
            tile.insert(TileFeaturePaintComponent::default());
        }
    }
}

pub(crate) fn begin(world: &mut World, coords: WorldTileCoords, requested: Vec<String>) {
    let vector = world.tiles.query::<&VectorLayerBucketComponent>(coords);
    let keep_vector = vector.is_some_and(|tile| tile.done && !tile.failed);
    if vector.is_none() {
        if let Some(mut tile) = world.tiles.spawn_mut(coords) {
            tile.insert(VectorLayerBucketComponent::default());
        }
    }
    let symbols = world.tiles.query::<&SymbolLayersDataComponent>(coords);
    let keep_symbols = symbols.is_some_and(|tile| !tile.pending_assets && !tile.layers.is_empty());
    if symbols.is_none() {
        if let Some(mut tile) = world.tiles.spawn_mut(coords) {
            tile.insert(SymbolLayersDataComponent::default());
        }
    }
    ensure(&mut world.tiles, coords);
    if let Some(replacements) = world.tiles.query_mut::<&mut LayerReplacements>(coords) {
        replacements.keep_vector = keep_vector;
        replacements.keep_symbols = keep_symbols;
        replacements.requested = requested;
        replacements.delivered.clear();
    }
}

/// Drops the layers the finished request was asked to build but did not deliver: hidden or
/// filtered out since the tile was last made, or left without features by new source data.
/// Buckets are otherwise only replaced by layer id, so such a layer would keep showing the
/// geometry of an earlier request for as long as the tile stays loaded.
pub(crate) fn drop_undelivered(world: &mut World, coords: WorldTileCoords) {
    let Some(pending) = world.tiles.query_mut::<&mut LayerReplacements>(coords) else {
        return;
    };
    let requested = std::mem::take(&mut pending.requested);
    let omitted: Vec<String> = requested
        .into_iter()
        .filter(|id| !pending.delivered.contains(id))
        .collect();
    if !omitted.is_empty() {
        purge_vector_layers(world, coords, &omitted);
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn accept_vector(world: &mut World, bucket: AvailableVectorLayerBucket) {
    accept_vector_with_paint(world, bucket, None);
}

pub(crate) fn accept_vector_with_paint(
    world: &mut World,
    bucket: AvailableVectorLayerBucket,
    feature_paint: Option<Vec<BucketFeatureEntry>>,
) {
    let coords = bucket.coords;
    if world
        .tiles
        .query::<&VectorLayerBucketComponent>(coords)
        .is_none()
    {
        return;
    }
    let uploaded = matches!(world.resources.get::<Eventually<VectorBufferPool>>(),
        Some(Eventually::Initialized(pool)) if pool.has_layer(coords, &bucket.style_layer_id));
    let clock = world.resources.get_or_init_mut::<ContentClock>();
    clock.0 = clock.0.wrapping_add(1);
    let revision = clock.0;
    ensure(&mut world.tiles, coords);
    let layer_id = bucket.style_layer_id.clone();
    if let Some((layers, pending)) = world
        .tiles
        .query_mut::<(&mut VectorLayerBucketComponent, &mut LayerReplacements)>(coords)
    {
        pending.revision = revision;
        if !pending.delivered.contains(&layer_id) {
            pending.delivered.push(layer_id.clone());
        }
        pending
            .vector
            .retain(|layer| layer.style_layer_id != layer_id);
        if uploaded {
            pending.vector.push(bucket);
        } else {
            commit_vector(layers, bucket);
        }
    }
    if let Some(paint_store) = world
        .tiles
        .query_mut::<&mut TileFeaturePaintComponent>(coords)
    {
        if uploaded {
            if let Some(entries) = feature_paint {
                paint_store.pending.insert(layer_id, entries);
            } else {
                paint_store.pending.remove(&layer_id);
            }
        } else {
            paint_store.pending.remove(&layer_id);
            if let Some(entries) = feature_paint {
                paint_store.committed.insert(layer_id, entries);
            } else {
                paint_store.committed.remove(&layer_id);
            }
        }
    }
}

pub(crate) fn commit_vector(
    layers: &mut VectorLayerBucketComponent,
    bucket: AvailableVectorLayerBucket,
) {
    layers.layers.retain(|layer| match layer {
        VectorLayerBucket::AvailableLayer(layer) => layer.style_layer_id != bucket.style_layer_id,
        VectorLayerBucket::Missing(layer) => layer.source_layer != bucket.source_layer,
    });
    layers
        .layers
        .push(VectorLayerBucket::AvailableLayer(bucket));
}

pub(crate) fn commit_pending_vector(tiles: &mut Tiles, coords: WorldTileCoords, index: usize) {
    let promoted = tiles
        .query_mut::<(&mut VectorLayerBucketComponent, &mut LayerReplacements)>(coords)
        .and_then(|(layers, pending)| {
            (index < pending.vector.len()).then(|| {
                let replaced = pending.vector.remove(index);
                let id = replaced.style_layer_id.clone();
                commit_vector(layers, replaced);
                id
            })
        });
    if let Some(id) = promoted {
        if let Some(paint_store) = tiles.query_mut::<&mut TileFeaturePaintComponent>(coords) {
            if let Some(entries) = paint_store.pending.remove(&id) {
                paint_store.committed.insert(id, entries);
            } else {
                paint_store.committed.remove(&id);
            }
        }
    }
}

pub(crate) fn accept_symbols(world: &mut World, bucket: SymbolLayerData) {
    let coords = bucket.coords;
    if world
        .tiles
        .query::<&SymbolLayersDataComponent>(coords)
        .is_none()
    {
        return;
    }
    let uploaded = matches!(world.resources.get::<Eventually<SymbolBufferPool>>(),
        Some(Eventually::Initialized(pool)) if pool.has_layer(coords, &bucket.style_layer_id));
    ensure(&mut world.tiles, coords);
    if let Some((layers, pending)) = world
        .tiles
        .query_mut::<(&mut SymbolLayersDataComponent, &mut LayerReplacements)>(coords)
    {
        pending
            .symbols
            .retain(|layer| layer.style_layer_id != bucket.style_layer_id);
        if uploaded {
            pending.symbols.push(bucket);
        } else {
            commit_symbols(layers, bucket);
        }
    }
}

pub(crate) fn commit_symbols(layers: &mut SymbolLayersDataComponent, bucket: SymbolLayerData) {
    layers
        .layers
        .retain(|layer| layer.style_layer_id != bucket.style_layer_id);
    layers.layers.push(bucket);
}

pub(crate) fn vector_layers(
    tiles: &Tiles,
    coords: WorldTileCoords,
) -> Vec<&AvailableVectorLayerBucket> {
    let pending = tiles.query::<&LayerReplacements>(coords);
    let mut layers: Vec<_> = tiles
        .query::<&VectorLayerBucketComponent>(coords)
        .into_iter()
        .flat_map(|layers| &layers.layers)
        .filter_map(|layer| match layer {
            VectorLayerBucket::AvailableLayer(layer) => Some(layer),
            VectorLayerBucket::Missing(_) => None,
        })
        .collect();
    if let Some(pending) = pending {
        for layer in &pending.vector {
            layers.retain(|current| current.style_layer_id != layer.style_layer_id);
            layers.push(layer);
        }
    }
    layers
}

/// Drops everything the removed style layers left in loaded tiles: their buckets, pending
/// replacements and GPU buffers. Buckets are otherwise only ever replaced by layer id, so a layer
/// that is no longer in the style would keep its geometry until the tile is evicted.
pub(crate) fn purge_layers(world: &mut World, removed: &[String]) {
    let coords: Vec<WorldTileCoords> = world.tiles.tiles.values().map(|tile| tile.coords).collect();
    for at in &coords {
        purge_vector_layers(world, *at, removed);
        if let Some(component) = world.tiles.query_mut::<&mut SymbolLayersDataComponent>(*at) {
            component
                .layers
                .retain(|layer| !removed.contains(&layer.style_layer_id));
        }
        if let Some(pending) = world.tiles.query_mut::<&mut LayerReplacements>(*at) {
            pending
                .symbols
                .retain(|layer| !removed.contains(&layer.style_layer_id));
        }
    }
    if let Some(Eventually::Initialized(pool)) =
        world.resources.get_mut::<Eventually<SymbolBufferPool>>()
    {
        for at in &coords {
            for id in removed {
                pool.remove_layer(*at, id);
            }
        }
    }
}

/// Drops the buckets, pending replacements and GPU buffers of the non-symbol `layers` in one tile.
fn purge_vector_layers(world: &mut World, at: WorldTileCoords, layers: &[String]) {
    if let Some(component) = world.tiles.query_mut::<&mut VectorLayerBucketComponent>(at) {
        component.layers.retain(|layer| match layer {
            VectorLayerBucket::AvailableLayer(bucket) => !layers.contains(&bucket.style_layer_id),
            VectorLayerBucket::Missing(_) => true,
        });
    }
    if let Some(pending) = world.tiles.query_mut::<&mut LayerReplacements>(at) {
        pending
            .vector
            .retain(|layer| !layers.contains(&layer.style_layer_id));
    }
    if let Some(paint_store) = world.tiles.query_mut::<&mut TileFeaturePaintComponent>(at) {
        for id in layers {
            paint_store.committed.remove(id);
            paint_store.pending.remove(id);
        }
    }
    if let Some(Eventually::Initialized(pool)) =
        world.resources.get_mut::<Eventually<VectorBufferPool>>()
    {
        for id in layers {
            pool.remove_layer(at, id);
        }
    }
}

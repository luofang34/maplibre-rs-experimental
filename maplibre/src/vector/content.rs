//! Successful layer replacements wait for their GPU data before replacing displayed content.

use std::sync::Arc;

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

pub(crate) struct LayerReplacements {
    pub vector: Vec<AvailableVectorLayerBucket>,
    pub symbols: Vec<SymbolLayerData>,
    pub keep_vector: bool,
    pub keep_symbols: bool,
    pub revision: u64,
    /// Non-symbol style layers the request in flight was asked to build; its result is the
    /// tile's whole content for them, so those it leaves out are dropped when it finishes.
    pub requested: Arc<[String]>,
    /// Which of [`requested`](Self::requested) the request has delivered so far.
    pub delivered: Vec<String>,
}

impl Default for LayerReplacements {
    fn default() -> Self {
        Self {
            vector: Vec::new(),
            symbols: Vec::new(),
            keep_vector: false,
            keep_symbols: false,
            revision: 0,
            requested: Arc::from([]),
            delivered: Vec::new(),
        }
    }
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
}

pub(crate) fn begin(world: &mut World, coords: WorldTileCoords, requested: Arc<[String]>) {
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
    let requested = std::mem::replace(&mut pending.requested, Arc::from([]));
    let omitted: Vec<String> = requested
        .iter()
        .filter(|id| !pending.delivered.contains(id))
        .cloned()
        .collect();
    if !omitted.is_empty() {
        purge_vector_layers(world, coords, &omitted);
    }
}

/// Whether `coords` already holds committed resident layers usable if a refresh is cancelled.
pub(crate) fn has_committed_resident_layers(world: &World, coords: WorldTileCoords) -> bool {
    world
        .tiles
        .query::<&VectorLayerBucketComponent>(coords)
        .is_some_and(|v| v.done && !v.failed && !v.layers.is_empty())
        && world
            .tiles
            .query::<&SymbolLayersDataComponent>(coords)
            .is_none_or(|s| !s.pending_assets || !s.layers.is_empty())
}

/// Discards staged refresh data at `coords` after cancellation while preserving committed layers.
pub(crate) fn discard_pending_refresh(world: &mut World, coords: WorldTileCoords) {
    if let Some(pending) = world.tiles.query_mut::<&mut LayerReplacements>(coords) {
        pending.vector.clear();
        pending.symbols.clear();
        pending.requested = Arc::from([]);
        pending.delivered.clear();
        pending.keep_vector = false;
        pending.keep_symbols = false;
    }
    if let Some(paint_store) = world
        .tiles
        .query_mut::<&mut TileFeaturePaintComponent>(coords)
    {
        paint_store.pending.clear();
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
        layers.uploaded_stamp = None;
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
    let feature_paint = feature_paint.filter(|entries| !entries.is_empty());
    if feature_paint.is_some()
        && world
            .tiles
            .query::<&TileFeaturePaintComponent>(coords)
            .is_none()
    {
        if let Some(mut tile) = world.tiles.spawn_mut(coords) {
            tile.insert(TileFeaturePaintComponent::default());
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
    layers.uploaded_stamp = None;
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

pub(crate) fn release_committed_cpu_geometry(
    tiles: &mut Tiles,
    coords: WorldTileCoords,
    style_layer_id: &str,
    release_colors: bool,
) {
    let keep_feature_indices = tiles
        .query::<&TileFeaturePaintComponent>(coords)
        .is_some_and(|store| store.committed.contains_key(style_layer_id));
    let Some(layers) = tiles.query_mut::<&mut VectorLayerBucketComponent>(coords) else {
        return;
    };
    for layer in &mut layers.layers {
        if let VectorLayerBucket::AvailableLayer(avail) = layer {
            if avail.style_layer_id == style_layer_id && avail.buffer.usable_indices > 0 {
                avail.buffer.release_cpu_geometry();
                if release_colors {
                    if !keep_feature_indices {
                        avail.feature_indices = Vec::new();
                    }
                    avail.feature_colors = Vec::new();
                    avail.feature_sort_keys = Vec::new();
                }
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
        layers.uploaded_stamp = None;
        layers.atlas_zoom_q32 = None;
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
    layers.uploaded_stamp = None;
    layers.atlas_zoom_q32 = None;
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

/// Whether any committed vector bucket at `coords` released its CPU indices after upload and
/// subsequently had its GPU allocation evicted from [`VectorBufferPool`].
pub(crate) fn has_evicted_cpu_released_vector_layers(
    coords: WorldTileCoords,
    world: &World,
) -> bool {
    let Some(Eventually::Initialized(pool)) = world.resources.get::<Eventually<VectorBufferPool>>()
    else {
        return false;
    };
    if !pool.has_capacity() {
        return false;
    }
    let Some(layers) = world.tiles.query::<&VectorLayerBucketComponent>(coords) else {
        return false;
    };
    let pending = world.tiles.query::<&LayerReplacements>(coords);
    if layers
        .uploaded_stamp
        .is_some_and(|(_, eviction_gen)| eviction_gen == pool.eviction_generation())
        && pending.is_none_or(|p| p.vector.is_empty())
    {
        return false;
    }
    layers.layers.iter().any(|layer| match layer {
        VectorLayerBucket::AvailableLayer(bucket) => {
            bucket.buffer.usable_indices > 0
                && !bucket.buffer.has_cpu_indices()
                && !pool.has_layer(coords, &bucket.style_layer_id)
                && !pending.is_some_and(|p| {
                    p.vector
                        .iter()
                        .any(|item| item.style_layer_id == bucket.style_layer_id)
                })
        }
        VectorLayerBucket::Missing(_) => false,
    })
}

/// Whether `coords` holds CPU-released vector or symbol buckets whose GPU pool allocations were
/// evicted while no replacement is pending, requiring a fresh worker pass to restore geometry.
pub(crate) fn has_evicted_gpu_geometry(
    world: &World,
    style: &crate::style::Style,
    (vectors_disabled, symbols_disabled): (bool, bool),
    coords: WorldTileCoords,
) -> bool {
    let rev = style.revision();
    let pending = world.tiles.query::<&LayerReplacements>(coords);
    if !vectors_disabled {
        if let Some(Eventually::Initialized(pool)) =
            world.resources.get::<Eventually<VectorBufferPool>>()
        {
            if pool.has_capacity() {
                if let Some(layers) = world.tiles.query::<&VectorLayerBucketComponent>(coords) {
                    let vector_stamped = layers.uploaded_stamp
                        == Some((rev, pool.eviction_generation()))
                        && pending.is_none_or(|p| p.vector.is_empty());
                    if !vector_stamped {
                        let evicted_vector = layers.layers.iter().any(|layer| match layer {
                            VectorLayerBucket::AvailableLayer(bucket) => {
                                bucket.buffer.usable_indices > 0
                                    && !bucket.buffer.has_cpu_indices()
                                    && !pool.has_layer(coords, &bucket.style_layer_id)
                                    && !pending.is_some_and(|p| {
                                        p.vector.iter().any(|item| {
                                            item.style_layer_id == bucket.style_layer_id
                                        })
                                    })
                                    && style
                                        .layers
                                        .iter()
                                        .any(|l| l.id == bucket.style_layer_id && !l.is_hidden())
                            }
                            VectorLayerBucket::Missing(_) => false,
                        });
                        if evicted_vector {
                            return true;
                        }
                    }
                }
            }
        }
    }
    if !symbols_disabled {
        if let Some(Eventually::Initialized(pool)) =
            world.resources.get::<Eventually<SymbolBufferPool>>()
        {
            if pool.has_capacity() {
                if let Some(symbols) = world.tiles.query::<&SymbolLayersDataComponent>(coords) {
                    let symbol_stamped = symbols.uploaded_stamp
                        == Some((rev, pool.eviction_generation()))
                        && pending.is_none_or(|p| p.symbols.is_empty());
                    if !symbol_stamped {
                        let evicted_symbol = symbols.layers.iter().any(|layer| {
                            layer.buffer.usable_indices > 0
                                && layer.buffer.buffer.vertices.is_empty()
                                && !pool.has_layer(coords, &layer.style_layer_id)
                                && !pending.is_some_and(|p| {
                                    p.symbols
                                        .iter()
                                        .any(|item| item.style_layer_id == layer.style_layer_id)
                                })
                                && style
                                    .layers
                                    .iter()
                                    .any(|l| l.id == layer.style_layer_id && !l.is_hidden())
                        });
                        if evicted_symbol {
                            return true;
                        }
                    }
                }
            }
        }
    }
    false
}

mod purge;
pub(crate) use purge::purge_layers;
use purge::purge_vector_layers;

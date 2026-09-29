//! Successful layer replacements wait for their GPU data before replacing displayed content.

use super::{
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
}
impl TileComponent for LayerReplacements {}

#[derive(Default)]
struct ContentClock(u64);

pub(crate) fn ensure(tiles: &mut Tiles, coords: WorldTileCoords) {
    if tiles.exists(coords) && tiles.query::<&LayerReplacements>(coords).is_none() {
        if let Some(mut tile) = tiles.spawn_mut(coords) {
            tile.insert(LayerReplacements::default());
        }
    }
}

pub(crate) fn begin(world: &mut World, coords: WorldTileCoords) {
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
    }
}

pub(crate) fn accept_vector(world: &mut World, bucket: AvailableVectorLayerBucket) {
    let coords = bucket.coords;
    if world
        .tiles
        .query::<&VectorLayerBucketComponent>(coords)
        .is_none()
    {
        return;
    }
    let uploaded = matches!(world.resources.get::<Eventually<VectorBufferPool>>(),
        Some(Eventually::Initialized(pool)) if pool.get_loaded_style_layers_at(coords)
            .is_some_and(|layers| layers.contains(bucket.style_layer_id.as_str())));
    let clock = world.resources.get_or_init_mut::<ContentClock>();
    clock.0 = clock.0.wrapping_add(1);
    let revision = clock.0;
    ensure(&mut world.tiles, coords);
    if let Some((layers, pending)) = world
        .tiles
        .query_mut::<(&mut VectorLayerBucketComponent, &mut LayerReplacements)>(coords)
    {
        pending.revision = revision;
        if uploaded {
            pending
                .vector
                .retain(|layer| layer.style_layer_id != bucket.style_layer_id);
            pending.vector.push(bucket);
        } else {
            pending
                .vector
                .retain(|layer| layer.style_layer_id != bucket.style_layer_id);
            commit_vector(layers, bucket);
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
        Some(Eventually::Initialized(pool)) if pool.get_loaded_style_layers_at(coords)
            .is_some_and(|layers| layers.contains(bucket.style_layer_id.as_str())));
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

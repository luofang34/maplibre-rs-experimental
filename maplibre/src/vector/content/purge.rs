use crate::{
    coords::WorldTileCoords,
    render::eventually::Eventually,
    sdf::{SymbolBufferPool, SymbolLayersDataComponent},
    tcs::world::World,
    vector::{
        content::LayerReplacements, feature_paint::TileFeaturePaintComponent, VectorBufferPool,
        VectorLayerBucket, VectorLayerBucketComponent,
    },
};

/// Drops everything the removed style layers left in loaded tiles: their buckets, pending
/// replacements and GPU buffers. Buckets are otherwise only ever replaced by layer id, so a layer
/// that is no longer in the style would keep its geometry until the tile is evicted.
pub(crate) fn purge_layers(world: &mut World, removed: &[String]) {
    let coords: Vec<WorldTileCoords> = world.tiles.tiles.values().map(|tile| tile.coords).collect();
    for at in &coords {
        purge_vector_layers(world, *at, removed);
        if let Some(component) = world.tiles.query_mut::<&mut SymbolLayersDataComponent>(*at) {
            component.uploaded_stamp = None;
            component.atlas_zoom_q32 = None;
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
pub(super) fn purge_vector_layers(world: &mut World, at: WorldTileCoords, layers: &[String]) {
    if let Some(component) = world.tiles.query_mut::<&mut VectorLayerBucketComponent>(at) {
        component.uploaded_stamp = None;
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

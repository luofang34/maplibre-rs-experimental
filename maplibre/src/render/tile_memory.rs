//! Accounts for retained tile data, including symbol atlases shared by style layers.
use std::{collections::HashSet, mem::size_of, sync::Arc};

use rustc_hash::FxHashSet;

#[cfg(feature = "raster")]
use crate::raster::{RasterLayerData, RasterLayersDataComponent};
use crate::{
    coords::WorldTileCoords,
    sdf::SymbolLayersDataComponent,
    style::expression::FeatureProperties,
    tcs::tiles::Tiles,
    terrain::{DemLayersComponent, DemTileComponent},
    vector::{
        content::LayerReplacements,
        feature_paint::{BucketFeatureEntry, TileFeaturePaintComponent},
        AvailableVectorLayerBucket, VectorLayerBucket, VectorLayerBucketComponent,
    },
};

fn capacity<T>(values: &Vec<T>) -> usize {
    values.capacity().saturating_mul(size_of::<T>())
}

/// Bytes owned by this tile, including an estimate for nested metadata and its query index.
pub(crate) fn tile_bytes(tiles: &Tiles, coords: WorldTileCoords) -> usize {
    let vector = tiles
        .query::<&VectorLayerBucketComponent>(coords)
        .map_or(0, |bucket| {
            bucket
                .layers
                .iter()
                .map(|layer| match layer {
                    VectorLayerBucket::AvailableLayer(layer) => vector_bytes(layer),
                    VectorLayerBucket::Missing(_) => 0,
                })
                .sum::<usize>()
        });
    #[cfg(feature = "raster")]
    let raster = tiles
        .query::<&RasterLayersDataComponent>(coords)
        .map_or(0, |bucket| {
            bucket
                .layers
                .iter()
                .map(|layer| match layer {
                    RasterLayerData::Available(layer) => capacity(layer.image.as_raw()),
                    RasterLayerData::Missing(_) => 0,
                })
                .sum::<usize>()
        });
    #[cfg(not(feature = "raster"))]
    let raster = 0;
    let mut seen_dems = HashSet::new();
    let mut dem = 0;
    if let Some(DemTileComponent::Loaded(loaded)) = tiles.query::<&DemTileComponent>(coords) {
        if seen_dems.insert(Arc::as_ptr(&loaded.tile)) {
            dem += loaded.tile.pixels().len();
        }
    }
    if let Some(layers) = tiles.query::<&DemLayersComponent>(coords) {
        for (_, loaded) in layers.iter() {
            if seen_dems.insert(Arc::as_ptr(&loaded.tile)) {
                dem += loaded.tile.pixels().len();
            }
        }
    }
    let pending = tiles
        .query::<&LayerReplacements>(coords)
        .map_or(0, |pending| {
            pending.vector.iter().map(vector_bytes).sum::<usize>()
        });
    vector
        + pending
        + raster
        + dem
        + symbols(tiles, coords)
        + tiles.geometry_index.tile_bytes(coords)
        + feature_paint_bytes(tiles, coords)
}

fn feature_paint_bytes(tiles: &Tiles, coords: WorldTileCoords) -> usize {
    let Some(paint) = tiles.query::<&TileFeaturePaintComponent>(coords) else {
        return 0;
    };
    let mut seen_props: FxHashSet<*const FeatureProperties> = FxHashSet::default();
    tiles
        .geometry_index
        .record_tile_property_ptrs(coords, &mut seen_props);
    const ENTRY_OVERHEAD: usize = 2 * size_of::<String>() + 16;
    let mut total = 0_usize;
    for (layer_id, entries) in paint.committed.iter().chain(paint.pending.iter()) {
        total = total
            .saturating_add(size_of::<(String, Vec<BucketFeatureEntry>)>())
            .saturating_add(layer_id.capacity())
            .saturating_add(capacity(entries));
        for entry in entries {
            if seen_props.insert(Arc::as_ptr(&entry.properties)) {
                let props_bytes: usize = entry
                    .properties
                    .iter()
                    .map(|(k, v)| k.capacity() + value_bytes(v) + ENTRY_OVERHEAD)
                    .sum();
                total = total
                    .saturating_add(size_of::<FeatureProperties>())
                    .saturating_add(props_bytes);
            }
        }
    }
    total
}

fn symbols(tiles: &Tiles, coords: WorldTileCoords) -> usize {
    let mut atlases = HashSet::new();
    tiles
        .query::<&SymbolLayersDataComponent>(coords)
        .into_iter()
        .flat_map(|bucket| &bucket.layers)
        .chain(
            tiles
                .query::<&LayerReplacements>(coords)
                .into_iter()
                .flat_map(|pending| &pending.symbols),
        )
        .map(|layer| {
            let geometry = capacity(&layer.buffer.buffer.vertices)
                + capacity(&layer.buffer.buffer.indices)
                + capacity(&layer.features)
                + layer
                    .features
                    .iter()
                    .map(|feature| {
                        feature.str.capacity()
                            + feature.data.properties.capacity()
                                * size_of::<(String, crate::style::expression::Value)>()
                            + feature
                                .data
                                .properties
                                .iter()
                                .map(|(key, value)| key.capacity() + value_bytes(value))
                                .sum::<usize>()
                    })
                    .sum::<usize>();
            let atlas = layer
                .atlas
                .as_ref()
                .filter(|atlas| atlases.insert(Arc::as_ptr(atlas)))
                .map_or(0, |atlas| atlas.approximate_bytes());
            geometry + atlas
        })
        .sum()
}

#[cfg(test)]
mod tests;

fn value_bytes(value: &crate::style::expression::Value) -> usize {
    use crate::style::expression::Value;
    match value {
        Value::String(value) => value.capacity(),
        Value::Array(values) => capacity(values) + values.iter().map(value_bytes).sum::<usize>(),
        Value::Object(values) => values
            .iter()
            .map(|(key, value)| size_of::<(String, Value)>() + key.capacity() + value_bytes(value))
            .sum(),
        _ => 0,
    }
}

fn vector_bytes(layer: &AvailableVectorLayerBucket) -> usize {
    capacity(&layer.buffer.buffer.vertices)
        + capacity(&layer.buffer.buffer.indices)
        + capacity(&layer.feature_indices)
        + capacity(&layer.feature_colors)
        + capacity(&layer.feature_sort_keys)
}

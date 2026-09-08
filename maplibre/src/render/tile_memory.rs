//! Accounts for retained tile data, including symbol atlases shared by style layers.
use crate::{
    coords::WorldTileCoords,
    raster::{RasterLayerData, RasterLayersDataComponent},
    sdf::SymbolLayersDataComponent,
    tcs::tiles::Tiles,
    terrain::DemTileComponent,
    vector::{VectorLayerBucket, VectorLayerBucketComponent},
};
use std::{collections::HashSet, mem::size_of, sync::Arc};

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
                    VectorLayerBucket::AvailableLayer(layer) => {
                        capacity(&layer.buffer.buffer.vertices)
                            + capacity(&layer.buffer.buffer.indices)
                            + capacity(&layer.feature_indices)
                            + capacity(&layer.feature_colors)
                    }
                    VectorLayerBucket::Missing(_) => 0,
                })
                .sum::<usize>()
        });
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
    let dem = match tiles.query::<&DemTileComponent>(coords) {
        Some(DemTileComponent::Loaded(dem)) => dem.tile.pixels().len(),
        _ => 0,
    };
    vector + raster + dem + symbols(tiles, coords) + tiles.geometry_index.tile_bytes(coords)
}

fn symbols(tiles: &Tiles, coords: WorldTileCoords) -> usize {
    let Some(bucket) = tiles.query::<&SymbolLayersDataComponent>(coords) else {
        return 0;
    };
    let mut atlases = HashSet::new();
    bucket
        .layers
        .iter()
        .map(|layer| {
            let geometry = capacity(&layer.new_buffer.buffer.vertices)
                + capacity(&layer.new_buffer.buffer.indices)
                + capacity(&layer.buffer.buffer.vertices)
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

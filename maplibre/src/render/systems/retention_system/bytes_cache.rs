//! Per-tile CPU byte estimate cache so `Cleanup` avoids re-scanning unchanged cached tiles.

use std::{
    hash::{Hash, Hasher},
    sync::Arc,
};

use rustc_hash::{FxHashMap, FxHasher};

#[cfg(feature = "raster")]
use crate::raster::{RasterLayerData, RasterLayersDataComponent};
use crate::{
    coords::WorldTileCoords,
    io::geometry_index::TileIndex,
    render::tile_memory::tile_bytes,
    sdf::SymbolLayersDataComponent,
    tcs::tiles::Tiles,
    terrain::{DemLayersComponent, DemTileComponent},
    vector::{
        content::LayerReplacements, feature_paint::TileFeaturePaintComponent, VectorLayerBucket,
        VectorLayerBucketComponent,
    },
};

#[derive(Default)]
pub(super) struct TileBytesCache {
    entries: FxHashMap<WorldTileCoords, (u64, usize)>,
}

impl TileBytesCache {
    pub(super) fn get_or_compute(&mut self, tiles: &Tiles, coords: WorldTileCoords) -> usize {
        let stamp = tile_bytes_stamp(tiles, coords);
        if let Some(&(cached_stamp, cached_bytes)) = self.entries.get(&coords) {
            if cached_stamp == stamp {
                return cached_bytes;
            }
        }
        let bytes = tile_bytes(tiles, coords);
        self.entries.insert(coords, (stamp, bytes));
        bytes
    }

    pub(super) fn remove(&mut self, coords: &WorldTileCoords) {
        self.entries.remove(coords);
    }

    pub(super) fn prune_if_oversized(&mut self, tiles: &Tiles) {
        if self.entries.len() > tiles.tiles.len() + 32 {
            self.entries.retain(|coords, _| tiles.exists(*coords));
        }
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }
}

/// Zero-allocation structural digest covering every component read by [`tile_bytes`].
pub(super) fn tile_bytes_stamp(tiles: &Tiles, coords: WorldTileCoords) -> u64 {
    let mut h = FxHasher::default();
    hash_vector_and_raster(tiles, coords, &mut h);
    hash_dem_and_replacements(tiles, coords, &mut h);
    hash_symbols_paint_and_index(tiles, coords, &mut h);
    h.finish()
}

fn hash_vector_and_raster(tiles: &Tiles, coords: WorldTileCoords, h: &mut FxHasher) {
    if let Some(buckets) = tiles.query::<&VectorLayerBucketComponent>(coords) {
        1_u8.hash(h);
        buckets.done.hash(h);
        buckets.failed.hash(h);
        buckets.uploaded_stamp.hash(h);
        buckets.layers.len().hash(h);
        (buckets.layers.as_ptr() as usize).hash(h);
        for layer in &buckets.layers {
            match layer {
                VectorLayerBucket::AvailableLayer(avail) => {
                    1_u8.hash(h);
                    avail.buffer.buffer.vertices.capacity().hash(h);
                    avail.buffer.buffer.indices.capacity().hash(h);
                    avail.buffer.prepacked_bytes().hash(h);
                    avail.feature_indices.capacity().hash(h);
                    avail.feature_colors.capacity().hash(h);
                    avail.feature_sort_keys.capacity().hash(h);
                }
                VectorLayerBucket::Missing(_) => 0_u8.hash(h),
            }
        }
    }
    #[cfg(feature = "raster")]
    if let Some(raster) = tiles.query::<&RasterLayersDataComponent>(coords) {
        2_u8.hash(h);
        raster.layers.len().hash(h);
        for layer in &raster.layers {
            match layer {
                RasterLayerData::Available(avail) => {
                    1_u8.hash(h);
                    avail.image.as_raw().capacity().hash(h);
                    (avail.image.as_raw().as_ptr() as usize).hash(h);
                }
                RasterLayerData::Missing(_) => 0_u8.hash(h),
                RasterLayerData::Empty(_) => 2_u8.hash(h),
            }
        }
    }
}

fn hash_dem_and_replacements(tiles: &Tiles, coords: WorldTileCoords, h: &mut FxHasher) {
    if let Some(dem) = tiles.query::<&DemTileComponent>(coords) {
        3_u8.hash(h);
        match dem {
            DemTileComponent::Pending => 0_u8.hash(h),
            DemTileComponent::Loaded(loaded) => {
                1_u8.hash(h);
                (Arc::as_ptr(&loaded.tile) as usize).hash(h);
            }
            DemTileComponent::Missing => 2_u8.hash(h),
        }
    }
    if let Some(dem_layers) = tiles.query::<&DemLayersComponent>(coords) {
        4_u8.hash(h);
        let mut count = 0_usize;
        for (_, loaded) in dem_layers.iter() {
            count += 1;
            (Arc::as_ptr(&loaded.tile) as usize).hash(h);
        }
        count.hash(h);
    }
    if let Some(pending) = tiles.query::<&LayerReplacements>(coords) {
        5_u8.hash(h);
        pending.vector.len().hash(h);
        (pending.vector.as_ptr() as usize).hash(h);
        for layer in &pending.vector {
            layer.buffer.buffer.vertices.capacity().hash(h);
            layer.buffer.buffer.indices.capacity().hash(h);
            layer.buffer.prepacked_bytes().hash(h);
            layer.feature_colors.capacity().hash(h);
        }
        pending.symbols.len().hash(h);
        (pending.symbols.as_ptr() as usize).hash(h);
        for layer in &pending.symbols {
            layer.buffer.buffer.vertices.capacity().hash(h);
            layer.features.capacity().hash(h);
            layer
                .atlas
                .as_ref()
                .map_or(0_usize, |a| Arc::as_ptr(a) as usize)
                .hash(h);
        }
    }
}

fn hash_symbols_paint_and_index(tiles: &Tiles, coords: WorldTileCoords, h: &mut FxHasher) {
    if let Some(symbols) = tiles.query::<&SymbolLayersDataComponent>(coords) {
        6_u8.hash(h);
        symbols.layers.len().hash(h);
        (symbols.layers.as_ptr() as usize).hash(h);
        for layer in &symbols.layers {
            layer.buffer.buffer.vertices.capacity().hash(h);
            layer.buffer.buffer.indices.capacity().hash(h);
            layer.buffer.icon_bits_bytes().hash(h);
            layer.features.capacity().hash(h);
            layer
                .atlas
                .as_ref()
                .map_or(0_usize, |a| Arc::as_ptr(a) as usize)
                .hash(h);
        }
    }
    if let Some(paint) = tiles.query::<&TileFeaturePaintComponent>(coords) {
        7_u8.hash(h);
        paint.committed.len().hash(h);
        paint.pending.len().hash(h);
        for entries in paint.committed.values().chain(paint.pending.values()) {
            entries.capacity().hash(h);
            (entries.as_ptr() as usize).hash(h);
        }
    }
    tiles.geometry_index.tile_bytes(coords).hash(h);
    if let Some(sources) = tiles.geometry_index.tile_indexes(&coords) {
        for (_, idx) in sources {
            match idx {
                TileIndex::Spatial { tree } => {
                    1_u8.hash(h);
                    tree.size().hash(h);
                }
                TileIndex::Linear { list } => {
                    2_u8.hash(h);
                    list.len().hash(h);
                }
                TileIndex::LazyMvt(lazy) => {
                    3_u8.hash(h);
                    lazy.materialized_slice().map_or(0, |s| s.len()).hash(h);
                }
            }
        }
    }
}

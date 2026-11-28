//! Per-tile `Arc<FeatureProperties>` deduplication helpers for vector and index batches.

#[cfg(feature = "headless")]
use crate::sdf::SymbolLayerData;
use crate::{
    coords::WorldTileCoords,
    io::geometry_index::{IndexedGeometry, TileIndex},
    sdf::{SymbolLayerReplacements, SymbolLayersDataComponent},
    tcs::{property_interner::CoordsPropertyTable, tiles::Tiles},
    vector::feature_paint::{BucketFeatureEntry, TileFeaturePaintComponent},
};

#[derive(Default)]
pub(crate) struct TilePropertyInterner(crate::tcs::property_interner::TilePropertyInterner);

fn table_for<'a>(
    inner: &'a mut crate::tcs::property_interner::TilePropertyInterner,
    tiles: &Tiles,
    coords: WorldTileCoords,
) -> &'a mut CoordsPropertyTable {
    inner.table_for_with(tiles, coords, |table| {
        if let Some(store) = tiles.query::<&TileFeaturePaintComponent>(coords) {
            for entries in store.committed.values().chain(store.pending.values()) {
                for entry in entries {
                    table.seed_existing(&entry.properties);
                }
            }
        }
        if let Some(symbols) = tiles.query::<&SymbolLayersDataComponent>(coords) {
            for layer in &symbols.layers {
                for feature in &layer.features {
                    table.seed_existing(&feature.data.properties);
                }
            }
        }
        if let Some(pending) = tiles.query::<&SymbolLayerReplacements>(coords) {
            for layer in &pending.symbols {
                for feature in &layer.features {
                    table.seed_existing(&feature.data.properties);
                }
            }
        }
    })
}

impl TilePropertyInterner {
    /// Deduplicates `entries` against resident and earlier-batch `Arc<FeatureProperties>` at `coords`.
    pub fn intern_entries(
        &mut self,
        tiles: &Tiles,
        coords: WorldTileCoords,
        entries: &mut [BucketFeatureEntry],
    ) {
        let table = table_for(&mut self.0, tiles, coords);
        for entry in entries {
            table.intern(&mut entry.properties);
        }
    }

    /// Deduplicates symbol feature property maps against resident and batch-peer `Arc`s at `coords`.
    #[cfg(feature = "headless")]
    pub fn intern_symbol_bucket(&mut self, tiles: &Tiles, bucket: &mut SymbolLayerData) {
        if bucket.features.iter().all(|f| f.data.properties.is_empty()) {
            return;
        }
        let table = table_for(&mut self.0, tiles, bucket.coords);
        for feature in &mut bucket.features {
            table.intern(&mut feature.data.properties);
        }
    }

    /// Deduplicates `index` geometries against resident and earlier-batch `Arc<FeatureProperties>` at `coords`.
    pub fn intern_tile_index(
        &mut self,
        tiles: &Tiles,
        coords: WorldTileCoords,
        index: &mut TileIndex,
    ) {
        let table = table_for(&mut self.0, tiles, coords);
        let mut visit = |geom: &mut IndexedGeometry<f64>| table.intern(&mut geom.properties);
        match index {
            TileIndex::Spatial { tree } => tree.iter_mut().for_each(&mut visit),
            TileIndex::Linear { list } => list.iter_mut().for_each(&mut visit),
            TileIndex::LazyMvt(lazy) => {
                if let Some(list) = lazy.materialized_mut() {
                    list.iter_mut().for_each(&mut visit);
                }
            }
        }
    }
}

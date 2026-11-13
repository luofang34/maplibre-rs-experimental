//! Per-tile deduplication of decoded `Arc<FeatureProperties>` across wire messages so style layers
//! and the query index share one map per feature on single-threaded / Web builds just as on native.

use std::{
    hash::{Hash, Hasher},
    sync::Arc,
};

use rustc_hash::{FxHashMap, FxHasher};
use smallvec::SmallVec;

use super::{BucketFeatureEntry, TileFeaturePaintComponent};
use crate::{
    coords::WorldTileCoords,
    io::geometry_index::{IndexedGeometry, TileIndex},
    style::expression::{FeatureProperties, Value},
    tcs::tiles::Tiles,
};

/// Deduplicates `Arc<FeatureProperties>` within and across messages arriving for the same tile,
/// retaining the original `Arc` pointers for the batch duration so heap addresses cannot be reused.
#[derive(Default)]
pub(crate) struct TilePropertyInterner {
    tiles: FxHashMap<WorldTileCoords, CoordsPropertyTable>,
}

#[derive(Default)]
struct CoordsPropertyTable {
    seeded: bool,
    /// Maps input `Arc::as_ptr` -> `(keep_alive_original, canonical_arc)` for `O(1)` fast-path
    /// hits without ABA pointer-reuse hazards.
    by_ptr: FxHashMap<usize, (Arc<FeatureProperties>, Arc<FeatureProperties>)>,
    by_hash: FxHashMap<u64, SmallVec<[Arc<FeatureProperties>; 1]>>,
}

impl CoordsPropertyTable {
    fn seed_existing(&mut self, props: &Arc<FeatureProperties>) {
        let ptr = Arc::as_ptr(props) as usize;
        if self.by_ptr.contains_key(&ptr) {
            return;
        }
        let hash = hash_properties(props);
        let bucket = self.by_hash.entry(hash).or_default();
        if let Some(existing) = bucket.iter().find(|cand| ***cand == **props) {
            self.by_ptr
                .insert(ptr, (Arc::clone(props), Arc::clone(existing)));
        } else {
            bucket.push(Arc::clone(props));
            self.by_ptr
                .insert(ptr, (Arc::clone(props), Arc::clone(props)));
        }
    }

    fn intern(&mut self, props: &mut Arc<FeatureProperties>) {
        let ptr = Arc::as_ptr(props) as usize;
        if let Some((_, canonical)) = self.by_ptr.get(&ptr) {
            *props = Arc::clone(canonical);
            return;
        }
        let hash = hash_properties(props);
        let bucket = self.by_hash.entry(hash).or_default();
        if let Some(existing) = bucket.iter().find(|cand| ***cand == **props) {
            let canonical = Arc::clone(existing);
            self.by_ptr
                .insert(ptr, (Arc::clone(props), Arc::clone(&canonical)));
            *props = canonical;
        } else {
            let canonical = Arc::clone(props);
            bucket.push(Arc::clone(&canonical));
            self.by_ptr.insert(ptr, (Arc::clone(&canonical), canonical));
        }
    }
}

impl TilePropertyInterner {
    fn table_for(&mut self, tiles: &Tiles, coords: WorldTileCoords) -> &mut CoordsPropertyTable {
        let table = self.tiles.entry(coords).or_default();
        if !table.seeded {
            table.seeded = true;
            tiles
                .geometry_index
                .for_each_tile_property(coords, |props| table.seed_existing(props));
            if let Some(store) = tiles.query::<&TileFeaturePaintComponent>(coords) {
                for entries in store.committed.values().chain(store.pending.values()) {
                    for entry in entries {
                        table.seed_existing(&entry.properties);
                    }
                }
            }
        }
        table
    }

    /// Deduplicates `entries` against resident and earlier-batch `Arc<FeatureProperties>` at `coords`.
    pub fn intern_entries(
        &mut self,
        tiles: &Tiles,
        coords: WorldTileCoords,
        entries: &mut [BucketFeatureEntry],
    ) {
        let table = self.table_for(tiles, coords);
        for entry in entries {
            table.intern(&mut entry.properties);
        }
    }

    /// Deduplicates `index` geometries against resident and earlier-batch `Arc<FeatureProperties>` at `coords`.
    pub fn intern_tile_index(
        &mut self,
        tiles: &Tiles,
        coords: WorldTileCoords,
        index: &mut TileIndex,
    ) {
        let table = self.table_for(tiles, coords);
        let mut visit = |geom: &mut IndexedGeometry<f64>| table.intern(&mut geom.properties);
        match index {
            TileIndex::Spatial { tree } => tree.iter_mut().for_each(&mut visit),
            TileIndex::Linear { list } => list.iter_mut().for_each(&mut visit),
        }
    }
}

fn hash_properties(props: &FeatureProperties) -> u64 {
    let mut acc = (props.len() as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    for (key, value) in props {
        let mut entry = FxHasher::default();
        key.hash(&mut entry);
        hash_value(value, &mut entry);
        acc = acc.wrapping_add(entry.finish());
    }
    acc
}

fn hash_value(value: &Value, hasher: &mut FxHasher) {
    match value {
        Value::Null => 0_u8.hash(hasher),
        Value::Bool(flag) => {
            1_u8.hash(hasher);
            flag.hash(hasher);
        }
        Value::Number(num) => {
            2_u8.hash(hasher);
            let canonical = if *num == 0.0 { 0.0_f64 } else { *num };
            canonical.to_bits().hash(hasher);
        }
        Value::String(text) => {
            3_u8.hash(hasher);
            text.hash(hasher);
        }
        Value::Color(c) => {
            4_u8.hash(hasher);
            c.r.to_bits().hash(hasher);
            c.g.to_bits().hash(hasher);
            c.b.to_bits().hash(hasher);
            c.a.to_bits().hash(hasher);
        }
        Value::Array(items) => {
            5_u8.hash(hasher);
            items.len().hash(hasher);
            for item in items {
                hash_value(item, hasher);
            }
        }
        Value::Object(entries) => {
            6_u8.hash(hasher);
            let mut acc = (entries.len() as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
            for (k, v) in entries {
                let mut entry = FxHasher::default();
                k.hash(&mut entry);
                hash_value(v, &mut entry);
                acc = acc.wrapping_add(entry.finish());
            }
            acc.hash(hasher);
        }
        _ => 7_u8.hash(hasher),
    }
}

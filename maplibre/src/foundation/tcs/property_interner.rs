//! Per-tile deduplication of decoded `Arc<FeatureProperties>` across wire messages so style
//! layers and the query index share one map per feature on single-threaded / Web builds just as
//! on native.

use std::{
    hash::{Hash, Hasher},
    sync::Arc,
};

use rustc_hash::{FxHashMap, FxHasher};
use smallvec::SmallVec;

use crate::{
    coords::WorldTileCoords,
    style::expression::{empty_feature_properties, FeatureProperties, Value},
    tcs::tiles::Tiles,
};

/// Deduplicates `Arc<FeatureProperties>` within and across messages arriving for the same tile,
/// retaining the original `Arc` pointers for the batch duration so heap addresses cannot be reused.
#[derive(Default)]
pub(crate) struct TilePropertyInterner {
    tiles: FxHashMap<WorldTileCoords, CoordsPropertyTable>,
}

#[derive(Default)]
pub(crate) struct CoordsPropertyTable {
    seeded: bool,
    /// Maps input `Arc::as_ptr` -> `(keep_alive_original, canonical_arc)` for `O(1)` fast-path
    /// hits without ABA pointer-reuse hazards.
    by_ptr: FxHashMap<usize, (Arc<FeatureProperties>, Arc<FeatureProperties>)>,
    by_hash: FxHashMap<u64, SmallVec<[Arc<FeatureProperties>; 1]>>,
}

impl CoordsPropertyTable {
    fn ensure_seeded(
        &mut self,
        tiles: &Tiles,
        coords: WorldTileCoords,
        seed_extra: impl FnOnce(&mut Self),
    ) {
        if !self.seeded {
            self.seeded = true;
            tiles
                .as_ref()
                .for_each_tile_property(coords, |props| self.seed_existing(props));
            seed_extra(self);
        }
    }

    pub(crate) fn seed_existing(&mut self, props: &Arc<FeatureProperties>) {
        if props.is_empty() {
            return;
        }
        let ptr = Arc::as_ptr(props) as usize;
        if self.by_ptr.contains_key(&ptr) {
            return;
        }
        let hash = hash_properties(props);
        let bucket = self.by_hash.entry(hash).or_default();
        if let Some(existing) = find_matching_arc(bucket, props) {
            self.by_ptr
                .insert(ptr, (Arc::clone(props), Arc::clone(existing)));
        } else {
            bucket.push(Arc::clone(props));
            self.by_ptr
                .insert(ptr, (Arc::clone(props), Arc::clone(props)));
        }
    }

    pub(crate) fn intern(&mut self, props: &mut Arc<FeatureProperties>) {
        if props.is_empty() {
            *props = Arc::clone(empty_feature_properties());
            return;
        }
        let ptr = Arc::as_ptr(props) as usize;
        if let Some((_, canonical)) = self.by_ptr.get(&ptr) {
            *props = Arc::clone(canonical);
            return;
        }
        let hash = hash_properties(props);
        let bucket = self.by_hash.entry(hash).or_default();
        if let Some(existing) = find_matching_arc(bucket, props) {
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
    /// Returns the per-tile interning table, seeding `geometry_index` and calling `seed_extra`
    /// once on first access for `coords`.
    pub(crate) fn table_for_with(
        &mut self,
        tiles: &Tiles,
        coords: WorldTileCoords,
        seed_extra: impl FnOnce(&mut CoordsPropertyTable),
    ) -> &mut CoordsPropertyTable {
        let table = self.tiles.entry(coords).or_default();
        table.ensure_seeded(tiles, coords, seed_extra);
        table
    }
}

fn find_matching_arc<'a>(
    bucket: &'a [Arc<FeatureProperties>],
    target: &FeatureProperties,
) -> Option<&'a Arc<FeatureProperties>> {
    bucket.iter().find(|cand| ***cand == *target)
}

fn mix_property_count(len: usize) -> u64 {
    (len as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
}

fn hash_key_value_entry(key: &str, value: &Value) -> u64 {
    let mut entry = FxHasher::default();
    key.hash(&mut entry);
    hash_value(value, &mut entry);
    entry.finish()
}

fn canonicalize_f64_bits(num: f64) -> u64 {
    let canonical = if num == 0.0 { 0.0_f64 } else { num };
    canonical.to_bits()
}

fn hash_color_channels(r: f64, g: f64, b: f64, a: f64, hasher: &mut FxHasher) {
    r.to_bits().hash(hasher);
    g.to_bits().hash(hasher);
    b.to_bits().hash(hasher);
    a.to_bits().hash(hasher);
}

fn hash_properties(props: &FeatureProperties) -> u64 {
    let mut acc = mix_property_count(props.len());
    for (key, value) in props {
        acc = acc.wrapping_add(hash_key_value_entry(key, value));
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
            canonicalize_f64_bits(*num).hash(hasher);
        }
        Value::String(text) => {
            3_u8.hash(hasher);
            text.hash(hasher);
        }
        Value::Color(c) => {
            4_u8.hash(hasher);
            hash_color_channels(c.r, c.g, c.b, c.a, hasher);
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
            let mut acc = mix_property_count(entries.len());
            for (k, v) in entries {
                acc = acc.wrapping_add(hash_key_value_entry(k, v));
            }
            acc.hash(hasher);
        }
        _ => 7_u8.hash(hasher),
    }
}

//! Zero-clone AABB-filtered slicing of a cached ancestor MVT [`Tile`] into a descendant's grid.

use geozero::mvt::{tile, Tile};
use rustc_hash::FxHashMap;

use super::{zigzag_decode, Frame, Kind, BUFFER_DIVISOR, CLOSE_PATH, LINE_TO, MOVE_TO};
use crate::coords::WorldTileCoords;

/// Slices a borrowed `tile` at `from` into a newly constructed [`Tile`] in the grid of `to`,
/// rejecting out-of-window features directly on the raw zigzag command stream before decoding
/// coordinate vectors or cloning tags, and compacting surviving layer string/value tables.
pub fn slice_descendant(tile: &Tile, from: WorldTileCoords, to: WorldTileCoords) -> Tile {
    let levels = u32::from(u8::from(to.z).saturating_sub(u8::from(from.z))).min(31);
    if levels == 0 {
        return tile.clone();
    }
    let scale = 1_i64 << levels;
    let column = i64::from(to.x) - (i64::from(from.x) << levels);
    let row = i64::from(to.y) - (i64::from(from.y) << levels);
    let layers = tile
        .layers
        .iter()
        .map(|layer| slice_layer(layer, scale, column, row))
        .collect();
    Tile { layers }
}

fn slice_layer(layer: &tile::Layer, scale: i64, column: i64, row: i64) -> tile::Layer {
    let extent = i64::from(layer.extent.unwrap_or(4096)).max(1);
    let frame = Frame {
        scale,
        offset: (column * extent, row * extent),
        low: -extent / BUFFER_DIVISOR,
        high: extent + extent / BUFFER_DIVISOR,
    };

    let mut kept_raw: Vec<(&tile::Feature, Vec<u32>)> = Vec::new();
    for feature in &layer.features {
        let Some(kind) = Kind::of(feature) else {
            continue;
        };
        if !frame.geometry_may_intersect(&feature.geometry) {
            continue;
        }
        if let Some(geometry) = frame.cut_encoded(kind, &feature.geometry) {
            kept_raw.push((feature, geometry));
        }
    }

    if kept_raw.is_empty() {
        return tile::Layer {
            version: layer.version,
            name: layer.name.clone(),
            features: Vec::new(),
            keys: Vec::new(),
            values: Vec::new(),
            extent: layer.extent,
        };
    }

    let mut keys: Vec<String> = Vec::new();
    let mut values: Vec<tile::Value> = Vec::new();
    let mut key_remap: FxHashMap<u32, u32> = FxHashMap::default();
    let mut val_remap: FxHashMap<u32, u32> = FxHashMap::default();
    let mut features = Vec::with_capacity(kept_raw.len());

    for (feature, geometry) in kept_raw {
        let mut tags = Vec::with_capacity(feature.tags.len());
        for pair in feature.tags.chunks(2) {
            let [k, v] = pair else {
                tags.extend_from_slice(pair);
                continue;
            };
            let new_k = if let Some(key_str) = layer.keys.get(*k as usize) {
                *key_remap.entry(*k).or_insert_with(|| {
                    let idx = keys.len() as u32;
                    keys.push(key_str.clone());
                    idx
                })
            } else {
                u32::MAX
            };
            let new_v = if let Some(val) = layer.values.get(*v as usize) {
                *val_remap.entry(*v).or_insert_with(|| {
                    let idx = values.len() as u32;
                    values.push(val.clone());
                    idx
                })
            } else {
                u32::MAX
            };
            tags.push(new_k);
            tags.push(new_v);
        }
        features.push(tile::Feature {
            id: feature.id,
            tags,
            r#type: feature.r#type,
            geometry,
        });
    }

    tile::Layer {
        version: layer.version,
        name: layer.name.clone(),
        features,
        keys,
        values,
        extent: layer.extent,
    }
}

impl Frame {
    /// Zero-allocation AABB pre-filter over a raw MVT command stream.
    ///
    /// Because all four clipping half-spaces (`x >= low`, `x <= high`, `y >= low`, `y <= high`)
    /// are convex, any point, segment, or polygon ring whose placed vertices lie strictly outside
    /// one half-space has an empty intersection with `[low..=high] × [low..=high]`.
    pub(super) fn geometry_may_intersect(&self, geometry: &[u32]) -> bool {
        let mut cursor = (0_i64, 0_i64);
        let mut index = 0;
        let mut has_point = false;
        let (mut min_x, mut max_x) = (i64::MAX, i64::MIN);
        let (mut min_y, mut max_y) = (i64::MAX, i64::MIN);
        while index < geometry.len() {
            let word = geometry[index];
            index += 1;
            let (id, count) = (word & 7, (word >> 3) as usize);
            match id {
                MOVE_TO | LINE_TO => {
                    for _ in 0..count {
                        let (Some(&wx), Some(&wy)) = (geometry.get(index), geometry.get(index + 1))
                        else {
                            return false;
                        };
                        index += 2;
                        cursor = (cursor.0 + zigzag_decode(wx), cursor.1 + zigzag_decode(wy));
                        let (px, py) = self.place(cursor);
                        min_x = min_x.min(px);
                        max_x = max_x.max(px);
                        min_y = min_y.min(py);
                        max_y = max_y.max(py);
                        has_point = true;
                    }
                }
                CLOSE_PATH => {}
                _ => return false,
            }
        }
        has_point
            && max_x >= self.low
            && min_x <= self.high
            && max_y >= self.low
            && min_y <= self.high
    }
}

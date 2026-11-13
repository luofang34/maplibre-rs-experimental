//! Struct-of-arrays wire encoding of [`TileIndex`] for single-threaded worker transport.
//!
//! Multipart features without an explicit feature `id` share an `Arc<FeatureProperties>` pointer
//! across their geometry parts so `query_rendered_features` deduplicates them as one feature.
//! [`WireTileIndex`] deduplicates property maps by `Arc` pointer into `prop_offsets` /
//! `prop_blobs` and interns `source_layer` strings into `strings`, preserving `Arc` sharing across
//! FlatBuffer serialization boundaries with a constant number of vector allocations per tile.

#![deny(missing_docs)]

use std::{collections::BTreeMap, sync::Arc};

use geo_types::{Coord, LineString, Point, Polygon};
use rstar::RTree;
use rustc_hash::FxHashMap;

use super::{ExactGeometry, FeatureMeta, IndexedGeometry, TileIndex};
use crate::style::expression::{Color, FeatureProperties, Value};

const KIND_POINT: u8 = 0;
const KIND_LINESTRING: u8 = 1;
const KIND_POLYGON: u8 = 2;

const TAG_NULL: u8 = 0;
const TAG_FALSE: u8 = 1;
const TAG_TRUE: u8 = 2;
const TAG_NUMBER: u8 = 3;
const TAG_STRING: u8 = 4;
const TAG_COLOR: u8 = 5;
const TAG_ARRAY: u8 = 6;
const TAG_OBJECT: u8 = 7;

/// Packed descriptor for one geometry part in [`WireTileIndex`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WireGeometry {
    /// Feature identifier when `has_id` is true.
    pub id: u64,
    /// Index into [`WireTileIndex::strings`] naming the source layer.
    pub source_layer_idx: u32,
    /// Index into [`WireTileIndex::prop_offsets`] naming the shared property map.
    pub prop_group_idx: u32,
    /// Feature position in its source layer for top-down tie breaking.
    pub feature_index: u32,
    /// Start offset in [`WireTileIndex::ring_ends`] (used by polygons).
    pub ring_start: u32,
    /// Number of rings in [`WireTileIndex::ring_ends`] (`0` for points and line strings).
    pub ring_count: u32,
    /// Start coordinate-pair index in [`WireTileIndex::xy`] (`xy[2 * coord_start ..]`).
    pub coord_start: u32,
    /// Number of `(x, y)` coordinate pairs belonging to this geometry.
    pub coord_count: u32,
    /// Geometry kind tag (`0` = point, `1` = line string, `2` = polygon).
    pub kind: u8,
    /// Whether `id` carries an explicit feature id.
    pub has_id: bool,
}

/// Flat Struct-of-Arrays wire representation of a [`TileIndex`] for worker transport.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WireTileIndex {
    /// Whether the index uses an R-tree (`true`) or sequential list (`false`).
    pub is_spatial: bool,
    /// Interned `source_layer` names referenced by [`WireGeometry::source_layer_idx`].
    pub strings: Vec<String>,
    /// Concatenated binary property payloads for unique `Arc<FeatureProperties>` groups.
    pub prop_blobs: Vec<u8>,
    /// Byte offsets into `prop_blobs` (`len == group_count + 1`, starting at `0`).
    pub prop_offsets: Vec<u32>,
    /// Packed descriptors for every geometry part.
    pub geometries: Vec<WireGeometry>,
    /// Cumulative ring end coordinate counts (relative to `coord_start`) for polygons.
    pub ring_ends: Vec<u32>,
    /// Interleaved `[x0, y0, x1, y1, ...]` tile-local coordinates.
    pub xy: Vec<f64>,
}

impl WireTileIndex {
    /// Packs `index` into contiguous flat arrays while preserving shared `Arc<FeatureProperties>`
    /// groups and interned `source_layer` names.
    pub fn from_tile_index(index: &TileIndex) -> Self {
        let count = match index {
            TileIndex::Spatial { tree } => tree.size(),
            TileIndex::Linear { list } => list.len(),
        };
        let mut wire = Self {
            is_spatial: matches!(index, TileIndex::Spatial { .. }),
            strings: Vec::new(),
            prop_blobs: Vec::new(),
            prop_offsets: vec![0],
            geometries: Vec::with_capacity(count),
            ring_ends: Vec::new(),
            xy: Vec::with_capacity(count * 4),
        };
        let mut layer_map: FxHashMap<Arc<str>, u32> = FxHashMap::default();
        let mut prop_map: FxHashMap<usize, u32> = FxHashMap::default();

        let mut push = |geom: &IndexedGeometry<f64>| {
            let source_layer_idx =
                *layer_map
                    .entry(geom.source_layer.clone())
                    .or_insert_with(|| {
                        let idx = wire.strings.len() as u32;
                        wire.strings.push(geom.source_layer.as_ref().to_owned());
                        idx
                    });
            let prop_ptr = Arc::as_ptr(&geom.properties) as usize;
            let prop_group_idx = *prop_map.entry(prop_ptr).or_insert_with(|| {
                let idx = (wire.prop_offsets.len() - 1) as u32;
                encode_properties(&geom.properties, &mut wire.prop_blobs);
                wire.prop_offsets.push(wire.prop_blobs.len() as u32);
                idx
            });

            let coord_start = (wire.xy.len() / 2) as u32;
            let ring_start = wire.ring_ends.len() as u32;
            let (kind, ring_count, coord_count) = match &geom.exact {
                ExactGeometry::Point(pt) => {
                    wire.xy.push(pt.x());
                    wire.xy.push(pt.y());
                    (KIND_POINT, 0, 1)
                }
                ExactGeometry::LineString(line) => {
                    for c in &line.0 {
                        wire.xy.push(c.x);
                        wire.xy.push(c.y);
                    }
                    (KIND_LINESTRING, 0, line.0.len() as u32)
                }
                ExactGeometry::Polygon(poly) => {
                    let mut rel_coords = 0_u32;
                    for c in &poly.exterior().0 {
                        wire.xy.push(c.x);
                        wire.xy.push(c.y);
                        rel_coords += 1;
                    }
                    wire.ring_ends.push(rel_coords);
                    for hole in poly.interiors() {
                        for c in &hole.0 {
                            wire.xy.push(c.x);
                            wire.xy.push(c.y);
                            rel_coords += 1;
                        }
                        wire.ring_ends.push(rel_coords);
                    }
                    (KIND_POLYGON, 1 + poly.interiors().len() as u32, rel_coords)
                }
            };

            wire.geometries.push(WireGeometry {
                id: geom.id.unwrap_or(0),
                source_layer_idx,
                prop_group_idx,
                feature_index: geom.feature_index,
                ring_start,
                ring_count,
                coord_start,
                coord_count,
                kind,
                has_id: geom.id.is_some(),
            });
        };

        match index {
            TileIndex::Spatial { tree } => tree.iter().for_each(&mut push),
            TileIndex::Linear { list } => list.iter().for_each(&mut push),
        }
        wire
    }

    /// Reconstructs a [`TileIndex`] from flat wire arrays, bulk-loading an [`RTree`] when
    /// `is_spatial` is true.
    pub fn into_tile_index(self) -> TileIndex {
        let layers: Vec<Arc<str>> = self.strings.into_iter().map(Arc::from).collect();
        let mut props: Vec<Arc<FeatureProperties>> =
            Vec::with_capacity(self.prop_offsets.len().saturating_sub(1));
        for pair in self.prop_offsets.windows(2) {
            let start = pair[0] as usize;
            let end = pair[1] as usize;
            let decoded = self
                .prop_blobs
                .get(start..end)
                .and_then(decode_properties)
                .unwrap_or_default();
            props.push(Arc::new(decoded));
        }

        let total_coords = self.xy.len() / 2;
        let mut geometries = Vec::with_capacity(self.geometries.len());
        for g in self.geometries {
            let Some(source_layer) = layers.get(g.source_layer_idx as usize).cloned() else {
                continue;
            };
            let Some(properties) = props.get(g.prop_group_idx as usize).cloned() else {
                continue;
            };
            let coord_start = g.coord_start as usize;
            let coord_end = coord_start.saturating_add(g.coord_count as usize);
            if coord_end > total_coords {
                continue;
            }
            let meta = FeatureMeta {
                properties,
                source_layer,
                id: g.has_id.then_some(g.id),
                feature_index: g.feature_index,
            };
            let indexed = match g.kind {
                KIND_POINT if g.coord_count == 1 => {
                    let pt = Point::new(self.xy[2 * coord_start], self.xy[2 * coord_start + 1]);
                    IndexedGeometry::from_point(pt, meta)
                }
                KIND_LINESTRING => {
                    let line = slice_linestring(&self.xy, coord_start, coord_end);
                    IndexedGeometry::from_linestring(line, meta)
                }
                KIND_POLYGON if g.ring_count >= 1 => {
                    let ring_start = g.ring_start as usize;
                    let ring_end = ring_start.saturating_add(g.ring_count as usize);
                    let Some(ends) = self.ring_ends.get(ring_start..ring_end) else {
                        continue;
                    };
                    let mut prev = 0_usize;
                    let mut rings = Vec::with_capacity(ends.len());
                    let mut valid = true;
                    for &rel_end in ends {
                        let rel_end = rel_end as usize;
                        if rel_end < prev || coord_start + rel_end > coord_end {
                            valid = false;
                            break;
                        }
                        rings.push(slice_linestring(
                            &self.xy,
                            coord_start + prev,
                            coord_start + rel_end,
                        ));
                        prev = rel_end;
                    }
                    if !valid {
                        continue;
                    }
                    let mut iter = rings.into_iter();
                    let Some(exterior) = iter.next() else {
                        continue;
                    };
                    let interiors: Vec<LineString<f64>> = iter.collect();
                    IndexedGeometry::from_polygon(Polygon::new(exterior, interiors), meta)
                }
                _ => None,
            };
            if let Some(indexed) = indexed {
                geometries.push(indexed);
            }
        }

        if self.is_spatial {
            TileIndex::Spatial {
                tree: RTree::bulk_load(geometries),
            }
        } else {
            TileIndex::Linear { list: geometries }
        }
    }
}

fn slice_linestring(xy: &[f64], start_coord: usize, end_coord: usize) -> LineString<f64> {
    let mut coords = Vec::with_capacity(end_coord.saturating_sub(start_coord));
    for i in start_coord..end_coord {
        coords.push(Coord {
            x: xy[2 * i],
            y: xy[2 * i + 1],
        });
    }
    LineString::new(coords)
}

pub(crate) fn encode_properties(props: &FeatureProperties, out: &mut Vec<u8>) {
    out.extend_from_slice(&(props.len() as u32).to_le_bytes());
    for (key, value) in props {
        encode_str(key, out);
        encode_value(value, out);
    }
}

fn encode_str(s: &str, out: &mut Vec<u8>) {
    let bytes = s.as_bytes();
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(bytes);
}

fn encode_value(value: &Value, out: &mut Vec<u8>) {
    match value {
        Value::Null => out.push(TAG_NULL),
        Value::Bool(false) => out.push(TAG_FALSE),
        Value::Bool(true) => out.push(TAG_TRUE),
        Value::Number(n) => {
            out.push(TAG_NUMBER);
            out.extend_from_slice(&n.to_le_bytes());
        }
        Value::String(s) => {
            out.push(TAG_STRING);
            encode_str(s, out);
        }
        Value::Color(c) => {
            out.push(TAG_COLOR);
            for ch in [c.r, c.g, c.b, c.a] {
                out.extend_from_slice(&ch.to_le_bytes());
            }
        }
        Value::Array(items) => {
            out.push(TAG_ARRAY);
            out.extend_from_slice(&(items.len() as u32).to_le_bytes());
            for item in items {
                encode_value(item, out);
            }
        }
        Value::Object(map) => {
            out.push(TAG_OBJECT);
            out.extend_from_slice(&(map.len() as u32).to_le_bytes());
            for (k, v) in map {
                encode_str(k, out);
                encode_value(v, out);
            }
        }
        Value::Collator(_)
        | Value::Image(_)
        | Value::Formatted(_)
        | Value::Padding(_)
        | Value::NumberArray(_)
        | Value::ColorArray(_)
        | Value::ProjectionDefinition(_)
        | Value::VariableAnchorOffsetCollection(_) => {
            out.push(TAG_NULL);
        }
    }
}

fn decode_properties(mut buf: &[u8]) -> Option<FeatureProperties> {
    decode_properties_from(&mut buf)
}

pub(crate) fn decode_properties_from(buf: &mut &[u8]) -> Option<FeatureProperties> {
    let count = read_u32(buf)? as usize;
    let mut props = FeatureProperties::new();
    for _ in 0..count {
        let key = read_string(buf)?;
        let val = decode_value(buf)?;
        props.insert(key, val);
    }
    Some(props)
}

pub(crate) fn read_u8(buf: &mut &[u8]) -> Option<u8> {
    let (&byte, rest) = buf.split_first()?;
    *buf = rest;
    Some(byte)
}

pub(crate) fn read_u32(buf: &mut &[u8]) -> Option<u32> {
    if buf.len() < 4 {
        return None;
    }
    let (head, rest) = buf.split_at(4);
    *buf = rest;
    Some(u32::from_le_bytes(head.try_into().ok()?))
}

fn read_f64(buf: &mut &[u8]) -> Option<f64> {
    if buf.len() < 8 {
        return None;
    }
    let (head, rest) = buf.split_at(8);
    *buf = rest;
    Some(f64::from_le_bytes(head.try_into().ok()?))
}

fn read_string(buf: &mut &[u8]) -> Option<String> {
    let len = read_u32(buf)? as usize;
    if buf.len() < len {
        return None;
    }
    let (head, rest) = buf.split_at(len);
    *buf = rest;
    std::str::from_utf8(head).ok().map(str::to_owned)
}

fn decode_value(buf: &mut &[u8]) -> Option<Value> {
    match read_u8(buf)? {
        TAG_NULL => Some(Value::Null),
        TAG_FALSE => Some(Value::Bool(false)),
        TAG_TRUE => Some(Value::Bool(true)),
        TAG_NUMBER => Some(Value::Number(read_f64(buf)?)),
        TAG_STRING => Some(Value::String(read_string(buf)?)),
        TAG_COLOR => Some(Value::Color(Color::new(
            read_f64(buf)?,
            read_f64(buf)?,
            read_f64(buf)?,
            read_f64(buf)?,
        ))),
        TAG_ARRAY => {
            let len = read_u32(buf)? as usize;
            let mut items = Vec::with_capacity(len.min(1024));
            for _ in 0..len {
                items.push(decode_value(buf)?);
            }
            Some(Value::Array(items))
        }
        TAG_OBJECT => {
            let len = read_u32(buf)? as usize;
            let mut map = BTreeMap::new();
            for _ in 0..len {
                let k = read_string(buf)?;
                let v = decode_value(buf)?;
                map.insert(k, v);
            }
            Some(Value::Object(map))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests;

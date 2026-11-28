//! Struct-of-arrays wire encoding of [`TileIndex`] for single-threaded worker transport.
//!
//! Multipart features without an explicit feature `id` share an `Arc<FeatureProperties>` pointer
//! across their geometry parts so `query_rendered_features` deduplicates them as one feature.
//! [`WireTileIndex`] deduplicates property maps by `Arc` pointer into `prop_offsets` /
//! `prop_blobs` and interns `source_layer` strings into `strings`, preserving `Arc` sharing across
//! FlatBuffer serialization boundaries with a constant number of vector allocations per tile.

#![deny(missing_docs)]

use std::sync::Arc;

use geo_types::{Coord, LineString, Point, Polygon};
use rstar::RTree;
use rustc_hash::FxHashMap;

use super::{ExactGeometry, FeatureMeta, IndexedGeometry, TileIndex};
use crate::style::expression::FeatureProperties;

mod codec;
use codec::decode_properties;
pub(crate) use codec::{
    decode_properties_from, encode_properties, encode_str, read_f32, read_f64, read_string,
    read_u32, read_u64, read_u8,
};

const KIND_POINT: u8 = 0;
const KIND_LINESTRING: u8 = 1;
const KIND_POLYGON: u8 = 2;

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

fn lazy_mvt_marker(compressed: bool) -> &'static str {
    if compressed {
        "\u{0}mvtz"
    } else {
        "\u{0}mvt"
    }
}

fn push_ring_coords(coords: &[Coord<f64>], xy: &mut Vec<f64>) -> u32 {
    for c in coords {
        xy.push(c.x);
        xy.push(c.y);
    }
    coords.len() as u32
}

fn decode_polygon_rings(
    xy: &[f64],
    ends: &[u32],
    coord_start: usize,
    coord_end: usize,
) -> Option<(LineString<f64>, Vec<LineString<f64>>)> {
    let mut prev = 0_usize;
    let mut rings = Vec::with_capacity(ends.len());
    for &rel_end in ends {
        let rel_end = rel_end as usize;
        if rel_end < prev || coord_start + rel_end > coord_end {
            return None;
        }
        rings.push(slice_linestring(
            xy,
            coord_start + prev,
            coord_start + rel_end,
        ));
        prev = rel_end;
    }
    let mut iter = rings.into_iter();
    let exterior = iter.next()?;
    let interiors: Vec<LineString<f64>> = iter.collect();
    Some((exterior, interiors))
}

fn decode_prop_table(prop_offsets: &[u32], prop_blobs: &[u8]) -> Vec<Arc<FeatureProperties>> {
    prop_offsets
        .windows(2)
        .filter_map(|pair| {
            let [start, end] = pair else {
                return None;
            };
            let decoded = prop_blobs
                .get(*start as usize..*end as usize)
                .and_then(decode_properties)
                .unwrap_or_default();
            Some(Arc::new(decoded))
        })
        .collect()
}

impl WireTileIndex {
    fn push_geometry(
        &mut self,
        geom: &IndexedGeometry<f64>,
        layer_map: &mut FxHashMap<Arc<str>, u32>,
        prop_map: &mut FxHashMap<usize, u32>,
    ) {
        let source_layer_idx = *layer_map
            .entry(geom.source_layer.clone())
            .or_insert_with(|| {
                let idx = self.strings.len() as u32;
                self.strings.push(geom.source_layer.as_ref().to_owned());
                idx
            });
        let prop_ptr = Arc::as_ptr(&geom.properties) as usize;
        let prop_group_idx = *prop_map.entry(prop_ptr).or_insert_with(|| {
            let idx = (self.prop_offsets.len() - 1) as u32;
            encode_properties(&geom.properties, &mut self.prop_blobs);
            self.prop_offsets.push(self.prop_blobs.len() as u32);
            idx
        });

        let coord_start = (self.xy.len() / 2) as u32;
        let ring_start = self.ring_ends.len() as u32;
        let (kind, ring_count, coord_count) = match &geom.exact {
            ExactGeometry::Point(pt) => {
                self.xy.push(pt.x());
                self.xy.push(pt.y());
                (KIND_POINT, 0, 1)
            }
            ExactGeometry::LineString(line) => {
                let count = push_ring_coords(&line.0, &mut self.xy);
                (KIND_LINESTRING, 0, count)
            }
            ExactGeometry::Polygon(poly) => {
                let mut rel_coords = push_ring_coords(&poly.exterior().0, &mut self.xy);
                self.ring_ends.push(rel_coords);
                for hole in poly.interiors() {
                    rel_coords += push_ring_coords(&hole.0, &mut self.xy);
                    self.ring_ends.push(rel_coords);
                }
                (KIND_POLYGON, 1 + poly.interiors().len() as u32, rel_coords)
            }
        };

        self.geometries.push(WireGeometry {
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
    }

    /// Packs `index` into contiguous flat arrays while preserving shared `Arc<FeatureProperties>`
    /// groups and interned `source_layer` names.
    pub fn from_tile_index(index: &TileIndex) -> Self {
        if let TileIndex::LazyMvt(lazy) = index {
            if lazy.materialized_slice().is_none() {
                let wire = lazy.wire();
                return Self {
                    is_spatial: false,
                    strings: vec![lazy_mvt_marker(wire.compressed).to_owned()],
                    prop_blobs: wire.bytes.to_vec(),
                    prop_offsets: Vec::new(),
                    geometries: Vec::new(),
                    ring_ends: Vec::new(),
                    xy: Vec::new(),
                };
            }
        }
        let count = match index {
            TileIndex::Spatial { tree } => tree.size(),
            TileIndex::Linear { list } => list.len(),
            TileIndex::LazyMvt(lazy) => lazy
                .materialized_slice()
                .map_or(0, <[IndexedGeometry<f64>]>::len),
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
        match index {
            TileIndex::Spatial { tree } => {
                tree.iter()
                    .for_each(|geom| wire.push_geometry(geom, &mut layer_map, &mut prop_map));
            }
            TileIndex::Linear { list } => {
                list.iter()
                    .for_each(|geom| wire.push_geometry(geom, &mut layer_map, &mut prop_map));
            }
            TileIndex::LazyMvt(lazy) => {
                lazy.materialized_slice()
                    .into_iter()
                    .flatten()
                    .for_each(|geom| wire.push_geometry(geom, &mut layer_map, &mut prop_map));
            }
        }
        wire
    }

    fn decode_geometry(
        g: &WireGeometry,
        xy: &[f64],
        ring_ends: &[u32],
        meta: FeatureMeta,
    ) -> Option<IndexedGeometry<f64>> {
        let total_coords = xy.len() / 2;
        let coord_start = g.coord_start as usize;
        let coord_end = coord_start.saturating_add(g.coord_count as usize);
        if coord_end > total_coords {
            return None;
        }
        match g.kind {
            KIND_POINT if g.coord_count == 1 => {
                let x = *xy.get(2 * coord_start)?;
                let y = *xy.get(2 * coord_start + 1)?;
                IndexedGeometry::from_point(Point::new(x, y), meta)
            }
            KIND_LINESTRING => {
                let line = slice_linestring(xy, coord_start, coord_end);
                IndexedGeometry::from_linestring(line, meta)
            }
            KIND_POLYGON if g.ring_count >= 1 => {
                let ring_start = g.ring_start as usize;
                let ring_end = ring_start.saturating_add(g.ring_count as usize);
                let ends = ring_ends.get(ring_start..ring_end)?;
                let (exterior, interiors) = decode_polygon_rings(xy, ends, coord_start, coord_end)?;
                IndexedGeometry::from_polygon(Polygon::new(exterior, interiors), meta)
            }
            _ => None,
        }
    }

    /// Reconstructs a [`TileIndex`] from flat wire arrays, bulk-loading an [`RTree`] when
    /// `is_spatial` is true.
    pub fn into_tile_index(self) -> TileIndex {
        if self.prop_offsets.is_empty() {
            match self.strings.first().map(String::as_str) {
                Some("\u{0}mvtz") => {
                    return TileIndex::LazyMvt(super::LazyMvtIndex::from_packed(
                        Arc::from(self.prop_blobs),
                        true,
                    ));
                }
                Some("\u{0}mvt") => {
                    return TileIndex::LazyMvt(super::LazyMvtIndex::new(Arc::from(
                        self.prop_blobs,
                    )));
                }
                _ => {}
            }
        }
        let layers: Vec<Arc<str>> = self.strings.into_iter().map(Arc::from).collect();
        let props = decode_prop_table(&self.prop_offsets, &self.prop_blobs);

        let mut geometries = Vec::with_capacity(self.geometries.len());
        for g in self.geometries {
            let (Some(source_layer), Some(properties)) = (
                layers.get(g.source_layer_idx as usize).cloned(),
                props.get(g.prop_group_idx as usize).cloned(),
            ) else {
                continue;
            };
            let meta = FeatureMeta {
                properties,
                source_layer,
                id: g.has_id.then_some(g.id),
                feature_index: g.feature_index,
            };
            if let Some(indexed) = Self::decode_geometry(&g, &self.xy, &self.ring_ends, meta) {
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
    let slice = xy.get(start_coord * 2..end_coord * 2).unwrap_or(&[]);
    let coords = slice
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&[x, y]| Coord { x, y })
        .collect();
    LineString::new(coords)
}

#[cfg(test)]
mod tests;

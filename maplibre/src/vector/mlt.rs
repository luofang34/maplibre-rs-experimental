//! Direct zero-copy MapLibre Tile (MLT) columnar streaming and MVT fallback conversion.

mod stream;
mod tessellate;

use mlt_core::{Decoder, Layer, Parser, PropValue, TileFeature, TileLayer};
use rustc_hash::FxHashMap;
pub(crate) use tessellate::{index_mlt_bytes, process_mlt_tile_with_assets, visit_mlt_features};

/// Validates that `data` parses cleanly as one or more MLT layers without decoding column streams.
pub(crate) fn validate_layers(data: &[u8]) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if data.is_empty() {
        return Err("empty MLT payload".into());
    }
    let layers = Parser::default().parse_layers(data)?;
    if layers.iter().any(|l| matches!(l, Layer::Tag01(_))) {
        Ok(())
    } else {
        Err("MLT payload contains no Tag01 layers".into())
    }
}

/// Returns `true` when `data` is a valid non-empty MLT layer stream.
#[inline]
pub(crate) fn is_valid_mlt(data: &[u8]) -> bool {
    validate_layers(data).is_ok()
}

/// Decodes an MLT payload into a `geozero::mvt::Tile` using an O(1) value dictionary (used for
/// parent-tile rescaling and external MVT-model callers; native tile rendering streams columnar
/// `ParsedLayer01` directly into tessellators via `process_mlt_tile_with_assets`).
pub fn decode(data: &[u8]) -> Result<geozero::mvt::Tile, Box<dyn std::error::Error>> {
    let layers = Parser::default().parse_layers(data)?;
    let mut decoder = Decoder::default();
    let mut tile_layers = Vec::with_capacity(layers.len());
    for layer in layers {
        if let Layer::Tag01(layer) = layer {
            let tile_layer = layer.into_tile(&mut decoder)?;
            tile_layers.push(convert_layer(&tile_layer));
        }
    }
    Ok(geozero::mvt::Tile {
        layers: tile_layers,
    })
}

#[derive(PartialEq, Eq, Hash)]
enum DictKey {
    Bool(bool),
    Sint(i64),
    Uint(u64),
    F32(u32),
    F64(u64),
    Str(String),
}

pub(super) fn convert_layer(layer: &TileLayer) -> geozero::mvt::tile::Layer {
    let mut values = Vec::new();
    let mut dict: FxHashMap<DictKey, u32> = FxHashMap::default();
    let features = layer
        .features()
        .iter()
        .map(|feat| convert_feature(feat, layer.property_names(), &mut values, &mut dict))
        .collect();
    geozero::mvt::tile::Layer {
        version: 2,
        name: layer.name().to_string(),
        extent: Some(layer.extent().get()),
        keys: layer.property_names().to_vec(),
        values,
        features,
    }
}

fn convert_feature(
    feat: &TileFeature,
    property_names: &[String],
    values: &mut Vec<geozero::mvt::tile::Value>,
    dict: &mut FxHashMap<DictKey, u32>,
) -> geozero::mvt::tile::Feature {
    let (geom_type, geometry) = encode_geometry(feat.geometry());
    let mut tags = Vec::new();
    for (col_idx, prop) in feat.properties().iter().enumerate() {
        if col_idx < property_names.len() {
            if let Some((key, val)) = prop_to_dict_value(prop) {
                let val_idx = *dict.entry(key).or_insert_with(|| {
                    let idx = u32::try_from(values.len()).unwrap_or(u32::MAX);
                    values.push(val);
                    idx
                });
                tags.push(col_idx as u32);
                tags.push(val_idx);
            }
        }
    }
    geozero::mvt::tile::Feature {
        id: feat.id(),
        tags,
        r#type: Some(geom_type),
        geometry,
    }
}

fn encode_geometry(geom: &geo_types::Geometry<i32>) -> (i32, Vec<u32>) {
    let mut out = Vec::new();
    let mut cursor = (0_i32, 0_i32);
    let geom_type = match geom {
        geo_types::Geometry::Point(p) => {
            encode_point(p, &mut out, &mut cursor);
            1
        }
        geo_types::Geometry::MultiPoint(mp) => {
            if !mp.0.is_empty() {
                out.push(((mp.0.len() as u32) << 3) | 1);
                for p in &mp.0 {
                    out.push(zigzag(p.x() - cursor.0));
                    out.push(zigzag(p.y() - cursor.1));
                    cursor = (p.x(), p.y());
                }
            }
            1
        }
        geo_types::Geometry::Line(l) => {
            encode_linestring(
                &geo_types::LineString::from(vec![l.start, l.end]),
                &mut out,
                &mut cursor,
            );
            2
        }
        geo_types::Geometry::LineString(ls) => {
            encode_linestring(ls, &mut out, &mut cursor);
            2
        }
        geo_types::Geometry::MultiLineString(mls) => {
            for ls in &mls.0 {
                encode_linestring(ls, &mut out, &mut cursor);
            }
            2
        }
        geo_types::Geometry::Polygon(poly) => {
            encode_polygon(poly, &mut out, &mut cursor);
            3
        }
        geo_types::Geometry::MultiPolygon(mpoly) => {
            for poly in &mpoly.0 {
                encode_polygon(poly, &mut out, &mut cursor);
            }
            3
        }
        geo_types::Geometry::Rect(r) => {
            encode_polygon(&r.to_polygon(), &mut out, &mut cursor);
            3
        }
        geo_types::Geometry::Triangle(t) => {
            encode_polygon(&t.to_polygon(), &mut out, &mut cursor);
            3
        }
        _ => 0,
    };
    (geom_type, out)
}

fn encode_point(p: &geo_types::Point<i32>, out: &mut Vec<u32>, cursor: &mut (i32, i32)) {
    out.push((1 << 3) | 1);
    out.push(zigzag(p.x() - cursor.0));
    out.push(zigzag(p.y() - cursor.1));
    *cursor = (p.x(), p.y());
}

fn encode_linestring(ls: &geo_types::LineString<i32>, out: &mut Vec<u32>, cursor: &mut (i32, i32)) {
    let Some(&first) = ls.0.first() else {
        return;
    };
    out.push((1 << 3) | 1);
    out.push(zigzag(first.x - cursor.0));
    out.push(zigzag(first.y - cursor.1));
    *cursor = (first.x, first.y);

    let count = ls.0.len() - 1;
    if count > 0 {
        out.push(((count as u32) << 3) | 2);
        for pt in ls.0.iter().skip(1) {
            out.push(zigzag(pt.x - cursor.0));
            out.push(zigzag(pt.y - cursor.1));
            *cursor = (pt.x, pt.y);
        }
    }
}

fn encode_polygon(poly: &geo_types::Polygon<i32>, out: &mut Vec<u32>, cursor: &mut (i32, i32)) {
    encode_ring(poly.exterior(), out, cursor);
    for interior in poly.interiors() {
        encode_ring(interior, out, cursor);
    }
}

fn encode_ring(ring: &geo_types::LineString<i32>, out: &mut Vec<u32>, cursor: &mut (i32, i32)) {
    let coords = match ring.0.as_slice() {
        [head @ .., tail] if !head.is_empty() && head.first() == Some(tail) => head,
        full => full,
    };
    let Some(&first) = coords.first() else {
        return;
    };
    if coords.len() < 3 {
        return;
    }
    out.push((1 << 3) | 1);
    out.push(zigzag(first.x - cursor.0));
    out.push(zigzag(first.y - cursor.1));
    *cursor = (first.x, first.y);

    let line_to_count = coords.len() - 1;
    out.push(((line_to_count as u32) << 3) | 2);
    for pt in coords.iter().skip(1) {
        out.push(zigzag(pt.x - cursor.0));
        out.push(zigzag(pt.y - cursor.1));
        *cursor = (pt.x, pt.y);
    }
    out.push((1 << 3) | 7);
}

#[inline]
fn zigzag(n: i32) -> u32 {
    ((n << 1) ^ (n >> 31)) as u32
}

fn dict_sint(v: i64) -> (DictKey, geozero::mvt::tile::Value) {
    (
        DictKey::Sint(v),
        geozero::mvt::tile::Value {
            sint_value: Some(v),
            ..Default::default()
        },
    )
}

fn dict_uint(v: u64) -> (DictKey, geozero::mvt::tile::Value) {
    (
        DictKey::Uint(v),
        geozero::mvt::tile::Value {
            uint_value: Some(v),
            ..Default::default()
        },
    )
}

fn prop_to_dict_value(prop: &PropValue) -> Option<(DictKey, geozero::mvt::tile::Value)> {
    match prop {
        PropValue::Bool(Some(b)) => Some((
            DictKey::Bool(*b),
            geozero::mvt::tile::Value {
                bool_value: Some(*b),
                ..Default::default()
            },
        )),
        PropValue::I8(Some(i)) => Some(dict_sint(i64::from(*i))),
        PropValue::I32(Some(i)) => Some(dict_sint(i64::from(*i))),
        PropValue::I64(Some(i)) => Some(dict_sint(*i)),
        PropValue::U8(Some(u)) => Some(dict_uint(u64::from(*u))),
        PropValue::U32(Some(u)) => Some(dict_uint(u64::from(*u))),
        PropValue::U64(Some(u)) => Some(dict_uint(*u)),
        PropValue::F32(Some(f)) => Some((
            DictKey::F32(f.to_bits()),
            geozero::mvt::tile::Value {
                float_value: Some(*f),
                ..Default::default()
            },
        )),
        PropValue::F64(Some(f)) => Some((
            DictKey::F64(f.to_bits()),
            geozero::mvt::tile::Value {
                double_value: Some(*f),
                ..Default::default()
            },
        )),
        PropValue::Str(Some(s)) => {
            let text = s.to_string();
            Some((
                DictKey::Str(text.clone()),
                geozero::mvt::tile::Value {
                    string_value: Some(text),
                    ..Default::default()
                },
            ))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests;

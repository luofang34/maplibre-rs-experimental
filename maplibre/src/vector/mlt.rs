//! Direct zero-copy MapLibre Tile (MLT) decoding into vector tile structures.

use mlt_core::{Decoder, Layer, Parser, PropValue, TileFeature, TileLayer};

/// Decodes an MLT payload directly into a vector tile without intermediate MVT protobuf re-encoding.
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

fn convert_layer(layer: &TileLayer) -> geozero::mvt::tile::Layer {
    let mut values = Vec::new();
    let features = layer
        .features()
        .iter()
        .map(|feat| convert_feature(feat, layer.property_names(), &mut values))
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
) -> geozero::mvt::tile::Feature {
    let (geom_type, geometry) = encode_geometry(feat.geometry());
    let mut tags = Vec::new();
    for (col_idx, prop) in feat.properties().iter().enumerate() {
        if col_idx < property_names.len() {
            if let Some(val) = prop_to_value(prop) {
                let val_idx = if let Some(idx) = values.iter().position(|v| v == &val) {
                    idx
                } else {
                    let idx = values.len();
                    values.push(val);
                    idx
                };
                tags.push(col_idx as u32);
                tags.push(val_idx as u32);
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
            let poly = geo_types::Polygon::new(
                geo_types::LineString::from(vec![
                    geo_types::Coord {
                        x: r.min().x,
                        y: r.min().y,
                    },
                    geo_types::Coord {
                        x: r.max().x,
                        y: r.min().y,
                    },
                    geo_types::Coord {
                        x: r.max().x,
                        y: r.max().y,
                    },
                    geo_types::Coord {
                        x: r.min().x,
                        y: r.max().y,
                    },
                    geo_types::Coord {
                        x: r.min().x,
                        y: r.min().y,
                    },
                ]),
                vec![],
            );
            encode_polygon(&poly, &mut out, &mut cursor);
            3
        }
        geo_types::Geometry::Triangle(t) => {
            let poly = geo_types::Polygon::new(
                geo_types::LineString::from(vec![t.v1(), t.v2(), t.v3(), t.v1()]),
                vec![],
            );
            encode_polygon(&poly, &mut out, &mut cursor);
            3
        }
        _ => 0,
    };
    (geom_type, out)
}

fn encode_point(p: &geo_types::Point<i32>, out: &mut Vec<u32>, cursor: &mut (i32, i32)) {
    out.push((1 << 3) | 1); // MoveTo, count 1
    out.push(zigzag(p.x() - cursor.0));
    out.push(zigzag(p.y() - cursor.1));
    *cursor = (p.x(), p.y());
}

fn encode_linestring(ls: &geo_types::LineString<i32>, out: &mut Vec<u32>, cursor: &mut (i32, i32)) {
    if ls.0.is_empty() {
        return;
    }
    out.push((1 << 3) | 1); // MoveTo, count 1
    let first = ls.0[0];
    out.push(zigzag(first.x - cursor.0));
    out.push(zigzag(first.y - cursor.1));
    *cursor = (first.x, first.y);

    let count = ls.0.len() - 1;
    if count > 0 {
        out.push(((count as u32) << 3) | 2); // LineTo, count
        for pt in &ls.0[1..] {
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
    let mut coords = ring.0.as_slice();
    if coords.len() > 1 && coords.first() == coords.last() {
        coords = &coords[..coords.len() - 1];
    }
    if coords.len() < 3 {
        return;
    }
    out.push((1 << 3) | 1); // MoveTo, count 1
    let first = coords[0];
    out.push(zigzag(first.x - cursor.0));
    out.push(zigzag(first.y - cursor.1));
    *cursor = (first.x, first.y);

    let line_to_count = coords.len() - 1;
    out.push(((line_to_count as u32) << 3) | 2); // LineTo
    for pt in &coords[1..] {
        out.push(zigzag(pt.x - cursor.0));
        out.push(zigzag(pt.y - cursor.1));
        *cursor = (pt.x, pt.y);
    }
    out.push((1 << 3) | 7); // ClosePath
}

#[inline]
fn zigzag(n: i32) -> u32 {
    ((n << 1) ^ (n >> 31)) as u32
}

fn prop_to_value(prop: &PropValue) -> Option<geozero::mvt::tile::Value> {
    match prop {
        PropValue::Bool(Some(b)) => Some(geozero::mvt::tile::Value {
            bool_value: Some(*b),
            ..Default::default()
        }),
        PropValue::I8(Some(i)) => Some(geozero::mvt::tile::Value {
            sint_value: Some(*i as i64),
            ..Default::default()
        }),
        PropValue::U8(Some(u)) => Some(geozero::mvt::tile::Value {
            uint_value: Some(*u as u64),
            ..Default::default()
        }),
        PropValue::I32(Some(i)) => Some(geozero::mvt::tile::Value {
            sint_value: Some(*i as i64),
            ..Default::default()
        }),
        PropValue::U32(Some(u)) => Some(geozero::mvt::tile::Value {
            uint_value: Some(*u as u64),
            ..Default::default()
        }),
        PropValue::I64(Some(i)) => Some(geozero::mvt::tile::Value {
            sint_value: Some(*i),
            ..Default::default()
        }),
        PropValue::U64(Some(u)) => Some(geozero::mvt::tile::Value {
            uint_value: Some(*u),
            ..Default::default()
        }),
        PropValue::F32(Some(f)) => Some(geozero::mvt::tile::Value {
            float_value: Some(*f),
            ..Default::default()
        }),
        PropValue::F64(Some(f)) => Some(geozero::mvt::tile::Value {
            double_value: Some(*f),
            ..Default::default()
        }),
        PropValue::Str(Some(s)) => Some(geozero::mvt::tile::Value {
            string_value: Some(s.to_string()),
            ..Default::default()
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests;

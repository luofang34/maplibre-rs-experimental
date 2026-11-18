//! Direct `&serde_json::Value` streamer for `geozero::FeatureProcessor` without `geojson` re-parsing.

use geozero::{
    error::GeozeroError, ColumnValue, FeatureProcessor, GeomProcessor, PropertyProcessor,
};
use serde_json::{Map, Value};

fn err(msg: impl Into<String>) -> GeozeroError {
    GeozeroError::Geometry(msg.into())
}

/// Streams a GeoJSON `FeatureCollection`, `Feature`, or bare geometry directly from `value` into
/// `processor`, emitting feature properties before geometry.
pub(crate) fn process_geojson_value<P: FeatureProcessor>(
    value: &Value,
    processor: &mut P,
) -> Result<(), GeozeroError> {
    let kind = value
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| err("GeoJSON object missing string `type`"))?;
    match kind {
        "FeatureCollection" => {
            let features = value
                .get("features")
                .and_then(Value::as_array)
                .ok_or_else(|| err("FeatureCollection missing `features` array"))?;
            processor.dataset_begin(None)?;
            for (idx, feature) in features.iter().enumerate() {
                emit_feature(feature, idx as u64, processor)?;
            }
            processor.dataset_end()?;
        }
        "Feature" => {
            processor.dataset_begin(None)?;
            emit_feature(value, 0, processor)?;
            processor.dataset_end()?;
        }
        _ => emit_geometry(value, processor, 0)?,
    }
    Ok(())
}

fn emit_feature<P: FeatureProcessor>(
    feature: &Value,
    idx: u64,
    processor: &mut P,
) -> Result<(), GeozeroError> {
    processor.feature_begin(idx)?;
    processor.properties_begin()?;
    if let Some(props) = feature.get("properties").and_then(Value::as_object) {
        emit_properties(props, processor)?;
    }
    processor.properties_end()?;
    processor.geometry_begin()?;
    if let Some(geometry) = feature.get("geometry").filter(|g| !g.is_null()) {
        emit_geometry(geometry, processor, 0)?;
    }
    processor.geometry_end()?;
    processor.feature_end(idx)?;
    Ok(())
}

fn emit_properties<P: PropertyProcessor>(
    props: &Map<String, Value>,
    processor: &mut P,
) -> Result<(), GeozeroError> {
    for (idx, (key, value)) in props.iter().enumerate() {
        let stop = match value {
            Value::Null => false,
            Value::Bool(flag) => processor.property(idx, key, &ColumnValue::Bool(*flag))?,
            Value::Number(num) => {
                if let Some(u) = num.as_u64() {
                    processor.property(idx, key, &ColumnValue::ULong(u))?
                } else if let Some(i) = num.as_i64() {
                    processor.property(idx, key, &ColumnValue::Long(i))?
                } else if let Some(f) = num.as_f64() {
                    processor.property(idx, key, &ColumnValue::Double(f))?
                } else {
                    false
                }
            }
            Value::String(text) => processor.property(idx, key, &ColumnValue::String(text))?,
            Value::Array(_) | Value::Object(_) => {
                let encoded = value.to_string();
                processor.property(idx, key, &ColumnValue::Json(&encoded))?
            }
        };
        if stop {
            break;
        }
    }
    Ok(())
}

fn emit_point_coords<P: GeomProcessor>(
    coord: &Value,
    idx: usize,
    processor: &mut P,
) -> Result<(), GeozeroError> {
    let arr = coord
        .as_array()
        .ok_or_else(|| err("expected coordinate array"))?;
    let x = arr
        .first()
        .and_then(Value::as_f64)
        .ok_or_else(|| err("coordinate missing x"))?;
    let y = arr
        .get(1)
        .and_then(Value::as_f64)
        .ok_or_else(|| err("coordinate missing y"))?;
    processor.xy(x, y, idx)
}

fn emit_coord_seq<P: GeomProcessor>(
    coords: &[Value],
    processor: &mut P,
) -> Result<(), GeozeroError> {
    for (i, pt) in coords.iter().enumerate() {
        emit_point_coords(pt, i, processor)?;
    }
    Ok(())
}

fn emit_rings<P: GeomProcessor>(rings: &[Value], processor: &mut P) -> Result<(), GeozeroError> {
    for (ri, ring) in rings.iter().enumerate() {
        let pts = ring
            .as_array()
            .ok_or_else(|| err("polygon ring is not an array"))?;
        processor.linestring_begin(false, pts.len(), ri)?;
        emit_coord_seq(pts, processor)?;
        processor.linestring_end(false, ri)?;
    }
    Ok(())
}

fn emit_geometry<P: GeomProcessor>(
    geometry: &Value,
    processor: &mut P,
    idx: usize,
) -> Result<(), GeozeroError> {
    let kind = geometry
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| err("geometry missing `type`"))?;
    if kind == "GeometryCollection" {
        let geoms = geometry
            .get("geometries")
            .and_then(Value::as_array)
            .ok_or_else(|| err("GeometryCollection missing `geometries`"))?;
        processor.geometrycollection_begin(geoms.len(), idx)?;
        for (gi, child) in geoms.iter().enumerate() {
            emit_geometry(child, processor, gi)?;
        }
        return processor.geometrycollection_end(idx);
    }
    let coords = geometry
        .get("coordinates")
        .ok_or_else(|| err(format!("{kind} missing `coordinates`")))?;
    match kind {
        "Point" => {
            processor.point_begin(idx)?;
            emit_point_coords(coords, 0, processor)?;
            processor.point_end(idx)?;
        }
        "MultiPoint" => {
            let pts = coords
                .as_array()
                .ok_or_else(|| err("MultiPoint coordinates not array"))?;
            processor.multipoint_begin(pts.len(), idx)?;
            emit_coord_seq(pts, processor)?;
            processor.multipoint_end(idx)?;
        }
        "LineString" => {
            let pts = coords
                .as_array()
                .ok_or_else(|| err("LineString coordinates not array"))?;
            processor.linestring_begin(true, pts.len(), idx)?;
            emit_coord_seq(pts, processor)?;
            processor.linestring_end(true, idx)?;
        }
        "MultiLineString" => {
            let lines = coords
                .as_array()
                .ok_or_else(|| err("MultiLineString coordinates not array"))?;
            processor.multilinestring_begin(lines.len(), idx)?;
            for (li, line) in lines.iter().enumerate() {
                let pts = line
                    .as_array()
                    .ok_or_else(|| err("MultiLineString element not array"))?;
                processor.linestring_begin(false, pts.len(), li)?;
                emit_coord_seq(pts, processor)?;
                processor.linestring_end(false, li)?;
            }
            processor.multilinestring_end(idx)?;
        }
        "Polygon" => {
            let rings = coords
                .as_array()
                .ok_or_else(|| err("Polygon coordinates not array"))?;
            processor.polygon_begin(true, rings.len(), idx)?;
            emit_rings(rings, processor)?;
            processor.polygon_end(true, idx)?;
        }
        "MultiPolygon" => {
            let polys = coords
                .as_array()
                .ok_or_else(|| err("MultiPolygon coordinates not array"))?;
            processor.multipolygon_begin(polys.len(), idx)?;
            for (pi, poly) in polys.iter().enumerate() {
                let rings = poly
                    .as_array()
                    .ok_or_else(|| err("MultiPolygon element not array"))?;
                processor.polygon_begin(false, rings.len(), pi)?;
                emit_rings(rings, processor)?;
                processor.polygon_end(false, pi)?;
            }
            processor.multipolygon_end(idx)?;
        }
        _ => return Err(err(format!("unsupported GeoJSON geometry type `{kind}`"))),
    }
    Ok(())
}

//! Reads GeoJSON coordinates into `[0..1]` Web Mercator world geometry and measures bounds.

use serde_json::Value;

use super::{Geometry, Point, Ring};

/// Mercator latitude limit in degrees.
const MAX_LATITUDE: f64 = 85.051_128_78;

fn project(position: &Value) -> Result<Point, &'static str> {
    let coordinates = position.as_array().ok_or("position is not an array")?;
    let longitude = coordinates
        .first()
        .and_then(Value::as_f64)
        .ok_or("position has no longitude")?;
    let latitude = coordinates
        .get(1)
        .and_then(Value::as_f64)
        .ok_or("position has no latitude")?;
    let latitude = latitude.clamp(-MAX_LATITUDE, MAX_LATITUDE).to_radians();
    Ok([
        (longitude + 180.0) / 360.0,
        0.5 - (std::f64::consts::FRAC_PI_4 + latitude / 2.0).tan().ln()
            / (2.0 * std::f64::consts::PI),
    ])
}

fn read_ring(value: &Value) -> Result<Ring, &'static str> {
    value
        .as_array()
        .ok_or("coordinates are not an array")?
        .iter()
        .map(project)
        .collect()
}

fn read_rings(value: &Value) -> Result<Vec<Ring>, &'static str> {
    value
        .as_array()
        .ok_or("coordinates are not an array")?
        .iter()
        .map(read_ring)
        .collect()
}

pub(super) fn read_geometry(geometry: &Value, out: &mut Geometry) -> Result<(), &'static str> {
    let coordinates = geometry.get("coordinates");
    let need = || coordinates.ok_or("geometry has no coordinates");
    match geometry.get("type").and_then(Value::as_str) {
        Some("Point") => out.points.push(project(need()?)?),
        Some("MultiPoint") => out.points.extend(read_ring(need()?)?),
        Some("LineString") => out.lines.push(read_ring(need()?)?),
        Some("MultiLineString") => out.lines.extend(read_rings(need()?)?),
        Some("Polygon") => out.polygons.push(read_rings(need()?)?),
        Some("MultiPolygon") => {
            for polygon in need()?.as_array().ok_or("coordinates are not an array")? {
                out.polygons.push(read_rings(polygon)?);
            }
        }
        Some("GeometryCollection") => {
            for member in geometry
                .get("geometries")
                .and_then(Value::as_array)
                .ok_or("collection has no geometries")?
            {
                read_geometry(member, out)?;
            }
        }
        _ => return Err("unsupported geometry type"),
    }
    Ok(())
}

fn positions(geometry: &Geometry) -> impl Iterator<Item = &Point> {
    geometry
        .points
        .iter()
        .chain(geometry.lines.iter().flatten())
        .chain(geometry.polygons.iter().flatten().flatten())
}

pub(super) fn coordinate_count(geometry: &Geometry) -> usize {
    positions(geometry).count()
}

pub(super) fn bounds_of(geometry: &Geometry) -> Option<[f64; 4]> {
    positions(geometry).fold(None, |bounds, point| {
        let [min_x, min_y, max_x, max_y] =
            bounds.unwrap_or([point[0], point[1], point[0], point[1]]);
        Some([
            min_x.min(point[0]),
            min_y.min(point[1]),
            max_x.max(point[0]),
            max_y.max(point[1]),
        ])
    })
}

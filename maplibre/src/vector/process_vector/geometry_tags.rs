//! Gives each feature of a tile layer its geometry in longitude and latitude, as a tag the
//! expression evaluator reads, for layers with `within` or `distance`.

use geo_types::Geometry as Geo;
use geozero::{mvt::tile, ToGeo};

use crate::{
    coords::WorldTileCoords,
    style::expression::{Geometry, GEOMETRY_PROPERTY},
};

/// Appends the geometry tag to every feature of `layer`.
pub(super) fn add_geometry_tags(layer: &mut tile::Layer, coords: WorldTileCoords) {
    let extent = f64::from(layer.extent.unwrap_or(4096)).max(1.0);
    let key = u32::try_from(layer.keys.len()).unwrap_or(u32::MAX);
    layer.keys.push(GEOMETRY_PROPERTY.to_owned());
    for feature in &mut layer.features {
        let Ok(geometry) = feature.to_geo() else {
            continue;
        };
        let text = lonlat(&geometry, coords, extent).to_geojson().to_string();
        let value = u32::try_from(layer.values.len()).unwrap_or(u32::MAX);
        layer.values.push(tile::Value {
            string_value: Some(text),
            ..Default::default()
        });
        feature.tags.extend([key, value]);
    }
}

fn lonlat(geometry: &Geo<f64>, coords: WorldTileCoords, extent: f64) -> Geometry {
    let tiles = 2_f64.powi(i32::from(u8::from(coords.z)));
    let convert = |c: &geo_types::Coord<f64>| {
        let x = (f64::from(coords.x) + c.x / extent) / tiles;
        let y = (f64::from(coords.y) + c.y / extent) / tiles;
        let longitude = x * 360.0 - 180.0;
        let latitude = (std::f64::consts::PI * (1.0 - 2.0 * y))
            .sinh()
            .atan()
            .to_degrees();
        [longitude, latitude]
    };
    let mut out = Geometry::default();
    collect(geometry, &convert, &mut out);
    out
}

fn collect(
    geometry: &Geo<f64>,
    convert: &dyn Fn(&geo_types::Coord<f64>) -> [f64; 2],
    out: &mut Geometry,
) {
    let line = |line: &geo_types::LineString<f64>| line.0.iter().map(convert).collect::<Vec<_>>();
    let polygon = |polygon: &geo_types::Polygon<f64>| {
        std::iter::once(polygon.exterior())
            .chain(polygon.interiors())
            .map(line)
            .collect::<Vec<_>>()
    };
    match geometry {
        Geo::Point(point) => out.points.push(convert(&point.0)),
        Geo::MultiPoint(points) => out.points.extend(points.0.iter().map(|p| convert(&p.0))),
        Geo::Line(segment) => out
            .lines
            .push(vec![convert(&segment.start), convert(&segment.end)]),
        Geo::LineString(string) => out.lines.push(line(string)),
        Geo::MultiLineString(strings) => out.lines.extend(strings.0.iter().map(line)),
        Geo::Polygon(p) => out.polygons.push(polygon(p)),
        Geo::MultiPolygon(polygons) => out.polygons.extend(polygons.0.iter().map(polygon)),
        Geo::GeometryCollection(parts) => {
            parts.0.iter().for_each(|part| collect(part, convert, out))
        }
        Geo::Rect(rect) => out.polygons.push(polygon(&rect.to_polygon())),
        Geo::Triangle(triangle) => out.polygons.push(polygon(&triangle.to_polygon())),
    }
}

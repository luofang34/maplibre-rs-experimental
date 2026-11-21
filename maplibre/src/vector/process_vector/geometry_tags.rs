//! Gives each feature of a tile layer its geometry in longitude and latitude, for layers with
//! `within` or `distance`, and its id and geometry type, for layers whose paint or layout reads
//! them: tags the expression evaluator reads.

use geo_types::Geometry as Geo;
use geozero::{mvt::tile, ToGeo};

use crate::{
    coords::WorldTileCoords,
    style::{
        expression::{Geometry, GEOMETRY_PROPERTY, GEOMETRY_TYPE_PROPERTY, ID_PROPERTY},
        filter::GeometryType,
    },
};

/// Appends the id and geometry type tags to every feature of `layer`; a feature without an id
/// gets none, so `["id"]` is null for it. When `promote_property` is set, the promoted property
/// replaces `feature.id` without falling back to it.
pub(super) fn add_identity_tags(layer: &mut tile::Layer, promote_property: Option<&str>) {
    let promote_key = promote_property.and_then(|prop| {
        layer
            .keys
            .iter()
            .position(|k| k == prop)
            .and_then(|idx| u32::try_from(idx).ok())
    });
    let key = |layer: &mut tile::Layer, name: &str| {
        layer.keys.push(name.to_owned());
        u32::try_from(layer.keys.len() - 1).unwrap_or(u32::MAX)
    };
    let (id_key, type_key) = (key(layer, ID_PROPERTY), key(layer, GEOMETRY_TYPE_PROPERTY));
    // One value per geometry type, shared by the features of that type.
    let types = [1, 2, 3, 0].map(|kind| {
        layer.values.push(tile::Value {
            string_value: Some(GeometryType::from_mvt(kind).name().to_owned()),
            ..Default::default()
        });
        (
            kind,
            u32::try_from(layer.values.len() - 1).unwrap_or(u32::MAX),
        )
    });
    for feature in &mut layer.features {
        let kind = feature.r#type.unwrap_or_default();
        let type_value = types
            .iter()
            .find(|(each, _)| *each == kind)
            .map_or(types[3].1, |(_, value)| *value);
        feature.tags.extend([type_key, type_value]);
        if promote_property.is_some() {
            if let Some(pk) = promote_key {
                let promoted_val = feature.tags.chunks_exact(2).find_map(|pair| {
                    if pair[0] != pk {
                        return None;
                    }
                    let val = layer.values.get(pair[1] as usize)?;
                    let is_valid_id = val.string_value.is_some()
                        || val.int_value.is_some()
                        || val.uint_value.is_some()
                        || val.sint_value.is_some()
                        || val.float_value.is_some()
                        || val.double_value.is_some();
                    is_valid_id.then_some(pair[1])
                });
                if let Some(val_idx) = promoted_val {
                    feature.tags.extend([id_key, val_idx]);
                }
            }
        } else if let Some(id) = feature.id {
            layer.values.push(tile::Value {
                uint_value: Some(id),
                ..Default::default()
            });
            let value = u32::try_from(layer.values.len() - 1).unwrap_or(u32::MAX);
            feature.tags.extend([id_key, value]);
        }
    }
}

#[cfg(test)]
mod tests {
    use geozero::mvt::tile;

    use super::*;

    #[test]
    fn add_identity_tags_uses_promoted_string_id_and_omits_missing() {
        let mut layer = tile::Layer {
            version: 2,
            name: "parks".to_owned(),
            keys: vec!["slug".to_owned()],
            values: vec![tile::Value {
                string_value: Some("central".to_owned()),
                ..Default::default()
            }],
            features: vec![
                tile::Feature {
                    id: Some(7),
                    tags: vec![0, 0],
                    r#type: Some(3),
                    geometry: vec![],
                },
                tile::Feature {
                    id: Some(8),
                    tags: vec![],
                    r#type: Some(3),
                    geometry: vec![],
                },
            ],
            extent: Some(4096),
        };
        add_identity_tags(&mut layer, Some("slug"));
        let id_key = layer
            .keys
            .iter()
            .position(|k| k == ID_PROPERTY)
            .expect("id key") as u32;
        let f0_id = layer.features[0]
            .tags
            .chunks_exact(2)
            .find_map(|pair| (pair[0] == id_key).then_some(pair[1]))
            .and_then(|idx| layer.values[idx as usize].string_value.as_deref());
        assert_eq!(f0_id, Some("central"));
        let f1_id = layer.features[1]
            .tags
            .chunks_exact(2)
            .find_map(|pair| (pair[0] == id_key).then_some(pair[1]));
        assert_eq!(f1_id, None, "missing promoted property must not fall back to feature.id");
    }
}

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

use serde_json::json;

use super::{Geometry, GEOMETRY_PROPERTY};
use crate::style::expression::{EvaluationContext, Expression, FeatureProperties};

fn square() -> Geometry {
    Geometry::from_geojson(
        &json!({"type": "Polygon", "coordinates": [[[0, 0], [0, 1], [1, 1], [1, 0], [0, 0]]]}),
    )
    .expect("polygon")
}

fn point(x: f64, y: f64) -> Geometry {
    Geometry::from_geojson(&json!({"type": "Point", "coordinates": [x, y]})).expect("point")
}

#[test]
fn a_point_is_within_a_polygon_only_inside_its_boundary() {
    assert!(point(0.5, 0.5).is_within(&square()));
    assert!(!point(1.5, 0.5).is_within(&square()));
    assert!(!point(1.0, 0.5).is_within(&square()));
}

#[test]
fn a_line_must_stay_inside_without_touching_the_boundary() {
    let line = |coordinates: serde_json::Value| {
        Geometry::from_geojson(&json!({"type": "LineString", "coordinates": coordinates}))
            .expect("line")
    };
    assert!(line(json!([[0.2, 0.2], [0.8, 0.8]])).is_within(&square()));
    assert!(!line(json!([[0.2, 0.2], [1.5, 0.5]])).is_within(&square()));
}

#[test]
fn distance_is_in_metres_and_zero_inside_a_polygon() {
    let metres = point(0.0, 0.0)
        .distance_to(&point(0.0, 0.001))
        .expect("distance");
    assert!((metres - 110.574).abs() < 0.5, "{metres}");
    assert_eq!(point(0.5, 0.5).distance_to(&square()), Some(0.0));
}

/// Evaluates `["distance", other]` for a feature of geometry `feature`, as a style does.
fn distance(feature: serde_json::Value, other: serde_json::Value) -> f64 {
    let properties =
        FeatureProperties::from([(GEOMETRY_PROPERTY.to_string(), feature.to_string().into())]);
    Expression::parse(&json!(["distance", other]))
        .expect("distance parses")
        .evaluate(&EvaluationContext::for_feature(0.0, &properties))
        .expect("distance evaluates")
        .as_number()
        .expect("distance is a number")
}

fn point_json(x: f64, y: f64) -> serde_json::Value {
    json!({"type": "Point", "coordinates": [x, y]})
}

/// Metres per degree of longitude and of latitude at `latitude`, by the WGS84 formula of
/// `CheapRuler`.
fn metres_per_degree(latitude: f64) -> [f64; 2] {
    let flattening = 1.0 / 298.257_223_563;
    let e2 = flattening * (2.0 - flattening);
    let cos = latitude.to_radians().cos();
    let w2 = 1.0 / (1.0 - e2 * (1.0 - cos * cos));
    let arc = std::f64::consts::PI / 180.0 * 6_378_137.0;
    [arc * w2.sqrt() * cos, arc * w2.sqrt() * w2 * (1.0 - e2)]
}

fn assert_metres(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 0.05,
        "{actual} m, expected {expected} m"
    );
}

#[test]
fn distance_follows_the_ellipsoid_of_cheap_ruler() {
    assert_metres(
        distance(point_json(0.0, 60.0), point_json(0.0, 60.01)),
        1_114.12,
    );
    assert_metres(
        distance(point_json(10.0, 0.0), point_json(10.2, 0.0)),
        22_263.90,
    );
}

#[test]
fn points_across_the_antimeridian_are_measured_the_short_way() {
    assert_metres(
        distance(point_json(179.9, 0.0), point_json(-179.9, 0.0)),
        22_263.90,
    );
    assert_metres(
        distance(point_json(-179.9, 0.0), point_json(179.9, 0.0)),
        22_263.90,
    );
}

#[test]
fn lines_across_the_antimeridian_stay_in_one_piece() {
    let crossing = json!({"type": "LineString", "coordinates": [[179.9, 0.0], [-179.9, 0.0]]});
    let expected = 0.1 * metres_per_degree(0.1)[1];
    assert_metres(
        distance(point_json(-179.95, 0.1), crossing.clone()),
        expected,
    );
    assert_metres(distance(point_json(180.0, 0.1), crossing.clone()), expected);
    let meridian = json!({"type": "LineString", "coordinates": [[-179.95, 0.1], [-179.95, 1.0]]});
    assert_metres(distance(meridian, crossing.clone()), expected);
    let touching = json!({"type": "LineString", "coordinates": [[179.95, -1.0], [179.95, 1.0]]});
    assert_eq!(distance(touching, crossing), 0.0);
}

#[test]
fn polygons_across_the_antimeridian_contain_points_on_both_sides() {
    let polygon = json!({"type": "Polygon", "coordinates": [[
        [179.5, -1.0], [-179.5, -1.0], [-179.5, 1.0], [179.5, 1.0], [179.5, -1.0]
    ]]});
    assert_eq!(distance(point_json(-179.9, 0.0), polygon.clone()), 0.0);
    assert_eq!(distance(point_json(179.9, 0.5), polygon.clone()), 0.0);
    assert_eq!(distance(point_json(180.0, 1.0), polygon.clone()), 0.0);
    assert_eq!(distance(polygon.clone(), point_json(-179.9, 0.0)), 0.0);
    assert_metres(
        distance(point_json(-179.0, 0.0), polygon),
        0.5 * metres_per_degree(0.0)[0],
    );
}

#[test]
fn geometry_across_the_antimeridian_is_far_from_a_feature_beside_it() {
    let crossing = json!({"type": "LineString", "coordinates": [[179.9, 0.0], [-179.9, 0.0]]});
    let polygon = json!({"type": "Polygon", "coordinates": [[
        [179.5, -1.0], [-179.5, -1.0], [-179.5, 1.0], [179.5, 1.0], [179.5, -1.0]
    ]]});
    let at_equator = metres_per_degree(0.0)[0];
    assert_metres(
        distance(point_json(0.0, 0.0), crossing.clone()),
        179.9 * at_equator,
    );
    assert_metres(
        distance(point_json(0.0, 0.0), polygon.clone()),
        179.5 * at_equator,
    );
    assert_metres(distance(crossing, point_json(0.0, 0.0)), 179.9 * at_equator);
    // The ruler takes the latitude of the feature's first point, the polygon's corner at -1°.
    assert_metres(
        distance(polygon, point_json(0.0, 0.0)),
        179.5 * metres_per_degree(-1.0)[0],
    );
}

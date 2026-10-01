use serde_json::json;

use super::Geometry;

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

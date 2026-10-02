use std::sync::Arc;

use geo_types::{line_string, polygon};

use super::*;

fn indexed(exact: ExactGeometry<f64>) -> IndexedGeometry<f64> {
    IndexedGeometry {
        bounds: rstar::AABB::from_corners(Point::new(-1e9, -1e9), Point::new(1e9, 1e9)),
        exact,
        properties: Arc::default(),
        source_layer: Arc::from("lines"),
        id: None,
    }
}

fn at(x: f64, y: f64) -> Footprint {
    Footprint::of(&[[x, y]]).unwrap()
}

#[test]
fn a_line_is_met_within_half_its_width() {
    let line = indexed(ExactGeometry::LineString(
        line_string![(x: 0.0, y: 0.0), (x: 100.0, y: 0.0)],
    ));

    assert!(touches(
        &line,
        &at(50.0, 4.0),
        ("line", [0.0; 2]),
        [5.0, 0.0]
    ));
    assert!(!touches(
        &line,
        &at(50.0, 6.0),
        ("line", [0.0; 2]),
        [5.0, 0.0]
    ));
}

#[test]
fn an_offset_line_is_met_where_it_is_drawn() {
    let line = indexed(ExactGeometry::LineString(
        line_string![(x: 0.0, y: 0.0), (x: 100.0, y: 0.0)],
    ));
    // GL JS offsets a line eastward along +y, to the right of its direction on a y-down grid.
    let drawn = at(50.0, 20.0);

    assert!(touches(&line, &drawn, ("line", [0.0; 2]), [2.0, 20.0]));
    assert!(!touches(&line, &drawn, ("line", [0.0; 2]), [2.0, 0.0]));
    assert!(!touches(
        &line,
        &at(50.0, 0.0),
        ("line", [0.0; 2]),
        [2.0, 20.0]
    ));
}

#[test]
fn a_corner_moves_along_its_bisector_by_the_full_offset_from_each_side() {
    let corner = line_string![(x: 0.0, y: 0.0), (x: 10.0, y: 0.0), (x: 10.0, y: 10.0)];
    let moved = offset_line(&corner, 1.0);
    let points: Vec<_> = moved.coords().map(|c| (c.x, c.y)).collect();

    assert_eq!(points[0], (0.0, 1.0));
    assert!((points[1].0 - 9.0).abs() < 1e-9 && (points[1].1 - 1.0).abs() < 1e-9);
    assert_eq!(points[2], (9.0, 10.0));
}

#[test]
fn a_translated_fill_is_met_where_it_is_drawn() {
    let square = indexed(ExactGeometry::Polygon(
        polygon![(x: 0.0, y: 0.0), (x: 10.0, y: 0.0), (x: 10.0, y: 10.0), (x: 0.0, y: 10.0)],
    ));

    assert!(touches(
        &square,
        &at(105.0, 5.0),
        ("fill", [100.0, 0.0]),
        [0.0; 2]
    ));
    assert!(!touches(
        &square,
        &at(5.0, 5.0),
        ("fill", [100.0, 0.0]),
        [0.0; 2]
    ));
}

#[test]
fn a_pitched_box_meets_only_what_its_trapezoid_covers() {
    // The trapezoid narrows towards the top; its bounding box would reach the square.
    let trapezoid = Footprint::of(&[[40.0, 0.0], [60.0, 0.0], [100.0, 50.0], [0.0, 50.0]]).unwrap();
    let square = indexed(ExactGeometry::Polygon(
        polygon![(x: 0.0, y: 0.0), (x: 10.0, y: 0.0), (x: 10.0, y: 10.0), (x: 0.0, y: 10.0)],
    ));

    assert!(!touches(&square, &trapezoid, ("fill", [0.0; 2]), [0.0; 2]));
}

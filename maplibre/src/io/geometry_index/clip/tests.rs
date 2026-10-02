use std::sync::Arc;

use geo_types::{coord, polygon, Coord, LineString, Rect};

use super::*;

fn meta() -> FeatureMeta {
    FeatureMeta {
        properties: Arc::default(),
        source_layer: Arc::from("_geojson"),
        id: Some(7),
        feature_index: 0,
    }
}

fn area() -> Rect<f64> {
    Rect::new(Coord { x: 0.0, y: 0.0 }, Coord { x: 10.0, y: 10.0 })
}

#[test]
fn a_polygon_keeps_only_its_part_within_the_area() {
    let square = polygon![(x: 5.0, y: 5.0), (x: 20.0, y: 5.0), (x: 20.0, y: 8.0), (x: 5.0, y: 8.0)];
    let parts = IndexedGeometry::from_polygon(square, meta())
        .unwrap()
        .clipped_to(area());
    assert_eq!(parts.len(), 1);
    let part = &parts[0];
    assert_eq!(part.bounds.upper().x(), 10.0);
    assert_eq!(part.bounds.lower().x(), 5.0);
    assert_eq!(part.id, Some(7));
}

#[test]
fn a_polygon_outside_the_area_leaves_nothing() {
    let square = polygon![(x: 20.0, y: 20.0), (x: 30.0, y: 20.0), (x: 30.0, y: 30.0)];
    let parts = IndexedGeometry::from_polygon(square, meta())
        .unwrap()
        .clipped_to(area());
    assert!(parts.is_empty());
}

#[test]
fn a_line_crossing_out_and_back_in_leaves_two_parts() {
    let line = LineString::new(vec![
        coord! { x: 2.0, y: 5.0 },
        coord! { x: 15.0, y: 5.0 },
        coord! { x: 15.0, y: 7.0 },
        coord! { x: 2.0, y: 7.0 },
    ]);
    let parts = IndexedGeometry::from_linestring(line, meta())
        .unwrap()
        .clipped_to(area());
    assert_eq!(parts.len(), 2);
    assert!(parts.iter().all(|part| part.bounds.upper().x() <= 10.0));
}

#[test]
fn a_polygon_outside_on_one_axis_but_spanning_the_other_leaves_nothing() {
    let strip =
        polygon![(x: -5.0, y: 20.0), (x: 15.0, y: 20.0), (x: 15.0, y: 30.0), (x: -5.0, y: 30.0)];
    let parts = IndexedGeometry::from_polygon(strip, meta())
        .unwrap()
        .clipped_to(area());
    assert!(parts.is_empty());
}

#[test]
fn a_hole_reaching_out_of_the_area_is_cut_with_it() {
    let outer = LineString::from(vec![
        (0.0, 0.0),
        (20.0, 0.0),
        (20.0, 20.0),
        (0.0, 20.0),
        (0.0, 0.0),
    ]);
    let hole = LineString::from(vec![
        (8.0, 4.0),
        (12.0, 4.0),
        (12.0, 6.0),
        (8.0, 6.0),
        (8.0, 4.0),
    ]);
    let parts = IndexedGeometry::from_polygon(Polygon::new(outer, vec![hole]), meta())
        .unwrap()
        .clipped_to(area());
    let ExactGeometry::Polygon(polygon) = &parts[0].exact else {
        panic!("a polygon stays a polygon");
    };
    assert_eq!(polygon.interiors().len(), 1);
    assert!(polygon.interiors()[0].coords().all(|c| c.x <= 10.0));
}

#[test]
fn a_ring_crossing_the_area_with_one_segment_keeps_both_crossings() {
    // One edge runs from left of the area to right of it.
    let wedge = polygon![(x: -5.0, y: 2.0), (x: 15.0, y: 2.0), (x: 5.0, y: 8.0)];
    let parts = IndexedGeometry::from_polygon(wedge, meta())
        .unwrap()
        .clipped_to(area());
    let ExactGeometry::Polygon(polygon) = &parts[0].exact else {
        panic!("a polygon stays a polygon");
    };
    let xs: Vec<f64> = polygon.exterior().coords().map(|c| c.x).collect();
    assert!(xs.contains(&0.0) && xs.contains(&10.0), "{xs:?}");
    assert!(polygon.exterior().is_closed());
}

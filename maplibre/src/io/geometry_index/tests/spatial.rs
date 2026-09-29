use std::collections::HashMap;

use geo_types::{LineString, Point, Polygon};
use rstar::{PointDistance, RTree};

use super::super::{IndexedGeometry, TileIndex};
use crate::coords::InnerCoords;

fn line(points: &[(f64, f64)], name: &str) -> IndexedGeometry<f64> {
    IndexedGeometry::from_linestring(
        LineString::from(points.to_vec()),
        HashMap::from([("name".into(), name.into())]),
    )
    .expect("nonempty line")
}

fn donut() -> IndexedGeometry<f64> {
    IndexedGeometry::from_polygon(
        Polygon::new(
            LineString::from(vec![
                (0.0, 0.0),
                (100.0, 0.0),
                (100.0, 100.0),
                (0.0, 100.0),
                (0.0, 0.0),
            ]),
            vec![LineString::from(vec![
                (20.0, 20.0),
                (20.0, 80.0),
                (80.0, 80.0),
                (80.0, 20.0),
                (20.0, 20.0),
            ])],
        ),
        HashMap::new(),
    )
    .expect("nonempty polygon")
}

#[test]
fn nearest_neighbor_uses_the_geometry_instead_of_its_bounds_center() {
    let tree = RTree::bulk_load(vec![
        line(&[(0.0, 1.0), (100.0, 1.0)], "near"),
        line(&[(-1.0, 5.0), (1.0, 5.0)], "far"),
    ]);
    let names: Vec<_> = tree
        .nearest_neighbor_iter(&Point::new(0.0, 0.0))
        .map(|geometry| geometry.properties.get("name").map(String::as_str))
        .collect();
    assert_eq!(names, vec![Some("near"), Some("far")]);
    let nearest = tree
        .nearest_neighbor(&Point::new(0.0, 0.0))
        .expect("nearest line");
    assert_eq!(
        nearest.properties.get("name").map(String::as_str),
        Some("near")
    );
    assert_eq!(nearest.distance_2(&Point::new(0.0, 0.0)), 1.0);
    let close: Vec<_> = tree
        .locate_within_distance(Point::new(0.0, 0.0), 4.0)
        .collect();
    assert_eq!(close.len(), 1);
    assert_eq!(
        close[0].properties.get("name").map(String::as_str),
        Some("near")
    );
}

#[test]
fn polygon_distance_and_contains_respect_the_hole_and_boundary() {
    let polygon = donut();
    assert_eq!(polygon.distance_2(&Point::new(10.0, 50.0)), 0.0);
    assert_eq!(polygon.distance_2(&Point::new(50.0, 50.0)), 900.0);
    assert_eq!(polygon.distance_2(&Point::new(110.0, 50.0)), 100.0);
    assert!(!polygon.contains_point(&Point::new(50.0, 50.0)));
    assert!(polygon.contains_point(&Point::new(0.0, 50.0)));
    assert!(
        !line(&[(0.0, 0.0), (100.0, 100.0)], "diagonal").contains_point(&Point::new(10.0, 80.0))
    );
}

#[test]
fn linear_and_spatial_queries_agree_on_holes_and_line_tolerance() {
    let list = vec![donut(), line(&[(200.0, 0.0), (300.0, 0.0)], "road")];
    let spatial = TileIndex::Spatial {
        tree: RTree::bulk_load(list.clone()),
    };
    let linear = TileIndex::Linear { list };
    for (x, y, count) in [
        (10.0, 50.0, 1),
        (50.0, 50.0, 0),
        (250.0, 8.0, 1),
        (250.0, 8.1, 0),
    ] {
        let query = InnerCoords { x, y };
        assert_eq!(linear.point_query(query).len(), count);
        assert_eq!(spatial.point_query(query).len(), count);
    }
}

use geo_types::{Coord, Rect};

use super::{line_within, ring_within};

fn coord(x: f64, y: f64) -> Coord<f64> {
    Coord { x, y }
}

fn area() -> Rect<f64> {
    Rect::new(coord(0.0, 0.0), coord(10.0, 10.0))
}

#[test]
fn a_ring_around_the_area_becomes_the_area() {
    let ring = [
        coord(-50.0, -50.0),
        coord(60.0, -50.0),
        coord(60.0, 60.0),
        coord(-50.0, 60.0),
        coord(-50.0, -50.0),
    ];
    let cut = ring_within(&ring, area());
    assert_eq!(cut.first(), cut.last(), "the cut ring is closed");
    assert_eq!(cut.len(), 5, "four corners and the closing point: {cut:?}");
    for corner in [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)] {
        assert!(
            cut.contains(&coord(corner.0, corner.1)),
            "{corner:?} in {cut:?}"
        );
    }
}

#[test]
fn a_ring_beside_the_area_leaves_nothing_to_draw() {
    let ring = [
        coord(20.0, 0.0),
        coord(30.0, 0.0),
        coord(30.0, 10.0),
        coord(20.0, 0.0),
    ];
    assert!(ring_within(&ring, area()).len() < 4);
}

#[test]
fn a_line_through_the_area_keeps_each_run_inside() {
    let line = vec![
        coord(-5.0, 5.0),
        coord(5.0, 5.0),
        coord(15.0, 5.0),
        coord(15.0, 8.0),
        coord(5.0, 8.0),
    ];
    let runs = line_within(line, area());
    assert_eq!(
        runs,
        vec![
            vec![coord(0.0, 5.0), coord(5.0, 5.0), coord(10.0, 5.0)],
            vec![coord(10.0, 8.0), coord(5.0, 8.0)],
        ]
    );
}

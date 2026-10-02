use cgmath::Deg;

use super::*;
use crate::{
    coords::{WorldCoords, Zoom},
    window::PhysicalSize,
};

const ZOOM: f64 = 15.0;

fn centre() -> f64 {
    256.0 * 2.0_f64.powf(ZOOM)
}

fn view(pitch: f64) -> ViewState {
    ViewState::new(
        PhysicalSize::new(800, 600).unwrap(),
        WorldCoords::at_ground(centre(), centre()),
        Zoom::new(ZOOM),
        Deg(pitch),
        Deg(36.87),
    )
}

/// A closed square ring `half` world pixels either side of `(x, y)`.
fn square(x: f64, y: f64, half: f64) -> Vec<[f64; 2]> {
    vec![
        [x - half, y - half],
        [x + half, y - half],
        [x + half, y + half],
        [x - half, y + half],
        [x - half, y - half],
    ]
}

fn block(half: f64) -> Vec<Vec<[f64; 2]>> {
    vec![square(centre(), centre(), half)]
}

#[test]
fn a_point_on_the_roof_meets_it_and_one_beside_it_misses() {
    let view = view(0.0);
    let on = ScreenQuery::Point([400.0, 300.0]);
    let beside = ScreenQuery::Point([700.0, 300.0]);

    assert!(intersection_depth(&view, &block(50.0), (0.0, 20.0), on).is_some());
    assert_eq!(
        intersection_depth(&view, &block(50.0), (0.0, 20.0), beside),
        None
    );
}

#[test]
fn a_taller_roof_is_nearer() {
    let view = view(0.0);
    let query = ScreenQuery::Point([400.0, 300.0]);

    let low = intersection_depth(&view, &block(50.0), (0.0, 10.0), query).unwrap();
    let high = intersection_depth(&view, &block(50.0), (0.0, 200.0), query).unwrap();

    assert!(high < low, "{high} should be nearer than {low}");
}

#[test]
fn a_pitched_extrusion_is_met_where_it_stands_up_above_its_footprint() {
    let view = view(60.0);
    let rings = block(50.0);
    let far_edge = |metres| project(&view, [centre(), centre() - 50.0], metres).unwrap();
    let (ground, roof) = (far_edge(0.0), far_edge(300.0));
    assert!(roof.y < ground.y, "the roof stands up the screen");
    let above_footprint = ScreenQuery::Point([ground.x, (ground.y + roof.y) / 2.0]);

    assert!(intersection_depth(&view, &rings, (0.0, 300.0), above_footprint).is_some());
    assert_eq!(
        intersection_depth(&view, &rings, (0.0, 0.0), above_footprint),
        None
    );
}

#[test]
fn a_box_takes_the_nearest_corner_of_what_it_covers() {
    let view = view(0.0);
    let rings = block(50.0);
    let corner = project(&view, [centre() - 50.0, centre() - 50.0], 20.0).unwrap();
    let all = ScreenQuery::Box([0.0, 0.0, 800.0, 600.0]);

    let depth = intersection_depth(&view, &rings, (0.0, 20.0), all).unwrap();

    assert!(
        (depth - corner.depth).abs() < 1e-9,
        "{depth} vs {}",
        corner.depth
    );
}

#[test]
fn a_point_in_a_hole_misses_the_roof() {
    let view = view(0.0);
    let rings = vec![
        square(centre(), centre(), 100.0),
        square(centre(), centre(), 40.0),
    ];
    let in_hole = ScreenQuery::Point([400.0, 300.0]);
    let on_roof = project(&view, [centre() + 70.0, centre()], 20.0).unwrap();

    assert_eq!(
        intersection_depth(&view, &rings, (0.0, 20.0), in_hole),
        None
    );
    assert!(intersection_depth(
        &view,
        &rings,
        (0.0, 20.0),
        ScreenQuery::Point([on_roof.x, on_roof.y])
    )
    .is_some());
}

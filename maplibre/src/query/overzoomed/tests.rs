use std::sync::Arc;

use geo_types::{line_string, Point};

use super::*;
use crate::io::geometry_index::ExactGeometry;

fn line(points: geo_types::LineString<f64>) -> IndexedGeometry<f64> {
    IndexedGeometry {
        bounds: rstar::AABB::from_points(points.points().collect::<Vec<Point<f64>>>().iter()),
        exact: ExactGeometry::LineString(points),
        properties: Arc::default(),
        source_layer: Arc::from("roads"),
        id: Some(1),
        feature_index: 3,
    }
}

#[test]
fn a_line_across_the_coarse_tile_is_cut_to_each_fine_tile_and_its_buffer() {
    // Across the coarse tile's middle, which is the edge between its two upper children.
    let across = line(line_string![(x: 1000.0, y: 1000.0), (x: 3000.0, y: 1000.0)]);
    let buffer = GL_BUFFER * EXTENT / 8192.0;

    let left = slice(&across, 2.0, [0.0, 0.0]);
    let right = slice(&across, 2.0, [EXTENT, 0.0]);

    assert_eq!(left.len(), 1);
    assert_eq!(
        left[0].bounds.upper().x(),
        EXTENT + buffer,
        "cut at the buffer"
    );
    assert_eq!(
        left[0].bounds.lower().x(),
        2000.0,
        "and moved into the child's units"
    );
    assert_eq!(right[0].bounds.lower().x(), -buffer);
    assert_eq!(right[0].feature_index, 3, "a part stays its feature");
}

#[test]
fn a_feature_only_in_the_buffer_of_a_fine_tile_is_left_to_its_neighbour() {
    // Ends just past the left child's right edge, inside its neighbour's buffer.
    let short = line(line_string![(x: 1000.0, y: 1000.0), (x: 2040.0, y: 1000.0)]);

    assert_eq!(slice(&short, 2.0, [0.0, 0.0]).len(), 1);
    assert!(slice(&short, 2.0, [EXTENT, 0.0]).is_empty());
}

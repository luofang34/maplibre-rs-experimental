use geo_types::{polygon, Point, Polygon};
use rstar::AABB;

use super::*;
use crate::io::geometry_index::ExactGeometry;

fn indexed(polygon: Polygon<f64>) -> IndexedGeometry<f64> {
    let bounds = AABB::from_points(
        polygon
            .exterior()
            .points()
            .collect::<Vec<Point<f64>>>()
            .iter(),
    );
    IndexedGeometry {
        bounds,
        exact: ExactGeometry::Polygon(polygon),
        properties: Default::default(),
        source_layer: GEOJSON_LAYER.into(),
        id: None,
    }
}

#[test]
fn a_feature_reaching_past_the_buffer_is_cut_at_it() {
    let wide = polygon![
        (x: 1000.0, y: 1000.0),
        (x: 9000.0, y: 1000.0),
        (x: 9000.0, y: 2000.0),
        (x: 1000.0, y: 2000.0),
    ];

    let parts = within_tile(vec![indexed(wide)], Some(128));

    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0].bounds.upper().x(), EXTENT + 128.0 * EXTENT / 512.0);
}

#[test]
fn a_feature_only_in_the_buffer_is_left_to_its_own_tile() {
    let beyond = EXTENT + 100.0;
    let neighbours = polygon![
        (x: 1000.0, y: beyond),
        (x: 2000.0, y: beyond),
        (x: 2000.0, y: beyond + 500.0),
        (x: 1000.0, y: beyond + 500.0),
    ];
    let straddling = polygon![
        (x: 1000.0, y: EXTENT - 100.0),
        (x: 2000.0, y: EXTENT - 100.0),
        (x: 2000.0, y: beyond),
        (x: 1000.0, y: beyond),
    ];

    let parts = within_tile(vec![indexed(neighbours), indexed(straddling)], Some(128));

    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0].bounds.lower().y(), EXTENT - 100.0);
}

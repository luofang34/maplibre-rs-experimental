#![allow(clippy::expect_used, clippy::panic)]

use geozero::{mvt::tile, GeozeroDatasource};

use super::IndexProcessor;

mod processing;
mod spatial;

fn zigzag(value: i64) -> u32 {
    ((value << 1) ^ (value >> 63)) as u32
}

/// A multi-point feature followed by a line, as a place layer next to a road layer would be.
fn multipoint_then_line() -> tile::Layer {
    let move_to_two = (2 << 3) | 1;
    let move_to_one = (1 << 3) | 1;
    let line_to_one = (1 << 3) | 2;
    tile::Layer {
        version: 2,
        name: "mixed".to_string(),
        features: vec![
            tile::Feature {
                id: Some(1),
                tags: Vec::new(),
                r#type: Some(tile::GeomType::Point as i32),
                geometry: vec![move_to_two, zigzag(10), zigzag(10), zigzag(5), zigzag(5)],
            },
            tile::Feature {
                id: Some(2),
                tags: Vec::new(),
                r#type: Some(tile::GeomType::Linestring as i32),
                geometry: vec![
                    move_to_one,
                    zigzag(0),
                    zigzag(0),
                    line_to_one,
                    zigzag(100),
                    zigzag(100),
                ],
            },
        ],
        extent: Some(4096),
        ..Default::default()
    }
}

#[test]
fn a_multipoint_feature_does_not_swallow_the_geometry_after_it() {
    let mut index = IndexProcessor::new();
    multipoint_then_line()
        .process(&mut index)
        .expect("the layer processes");

    let parts: Vec<_> = index
        .get_geometries()
        .into_iter()
        .map(|part| {
            let kind = match part.exact {
                super::ExactGeometry::Point(_) => "point",
                super::ExactGeometry::LineString(_) => "line",
                super::ExactGeometry::Polygon(_) => "polygon",
            };
            (kind, part.id, part.feature_index)
        })
        .collect();
    assert_eq!(
        parts,
        [("point", None, 0), ("point", None, 0), ("line", None, 1)],
        "each point of the multi-point is a part of the first feature, and the line follows"
    );
}

#[test]
fn a_bare_geometry_is_indexed_once_committed() {
    let mut index = IndexProcessor::new();
    geozero::geojson::GeoJson(r#"{"type": "Point", "coordinates": [5, 6]}"#)
        .process(&mut index)
        .expect("the geometry processes");
    index.commit_bare_geometry();

    let parts = index.get_geometries();
    assert_eq!(parts.len(), 1);
    assert!(matches!(parts[0].exact, super::ExactGeometry::Point(point) if point.x() == 5.0));
}

#![allow(clippy::expect_used, clippy::panic)]

use geozero::{mvt::tile, GeozeroDatasource};

use super::IndexProcessor;

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

    let geometries = index.get_geometries();
    assert_eq!(
        geometries.len(),
        1,
        "the line is indexed; points are not queryable"
    );
}

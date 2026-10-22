#![allow(clippy::expect_used, clippy::panic)]

use geozero::mvt::{tile, Tile};

use super::{decode, into_descendant, Kind};
use crate::coords::{WorldTileCoords, ZoomLevel};

fn coords(x: i32, y: i32, z: u8) -> WorldTileCoords {
    WorldTileCoords {
        x,
        y,
        z: ZoomLevel::new(z),
    }
}

fn feature(kind: tile::GeomType, geometry: Vec<u32>) -> tile::Feature {
    tile::Feature {
        id: Some(1),
        tags: vec![],
        r#type: Some(kind as i32),
        geometry,
    }
}

fn tile_of(features: Vec<tile::Feature>) -> Tile {
    Tile {
        layers: vec![tile::Layer {
            version: 2,
            name: "land".into(),
            features,
            keys: vec![],
            values: vec![],
            extent: Some(4096),
        }],
    }
}

fn zigzag(value: i64) -> u32 {
    super::zigzag(value)
}

/// A command stream from absolute points, as the encoder under test writes one.
fn encoded(kind: Kind, parts: &[Vec<(i64, i64)>]) -> Vec<u32> {
    super::encode(kind, parts)
}

fn parts(tile: &Tile) -> Vec<Vec<Vec<(i64, i64)>>> {
    tile.layers[0]
        .features
        .iter()
        .map(|feature| decode(&feature.geometry).expect("readable"))
        .collect()
}

/// Every feature of the tile is one a vector tile reader accepts.
fn readable(tile: &Tile) {
    use geozero::{GeozeroGeometry, ProcessorSink};
    for feature in &tile.layers[0].features {
        feature
            .process_geom(&mut ProcessorSink)
            .expect("the rescaled geometry can be read");
    }
}

#[test]
fn a_point_at_the_center_of_the_ancestor_lands_at_the_corner_each_child_shares() {
    let point = feature(tile::GeomType::Point, vec![9, zigzag(2048), zigzag(2048)]);
    let mut north_west = tile_of(vec![point.clone()]);
    into_descendant(&mut north_west, coords(0, 0, 0), coords(0, 0, 1));
    assert_eq!(parts(&north_west), vec![vec![vec![(4096, 4096)]]]);
    let mut south_east = tile_of(vec![point]);
    into_descendant(&mut south_east, coords(0, 0, 0), coords(1, 1, 1));
    assert_eq!(parts(&south_east), vec![vec![vec![(0, 0)]]]);
    readable(&north_west);
}

#[test]
fn a_feature_beyond_the_tile_and_its_buffer_is_left_out() {
    let point = feature(tile::GeomType::Point, vec![9, zigzag(100), zigzag(100)]);
    let mut far = tile_of(vec![point.clone()]);
    into_descendant(&mut far, coords(0, 0, 0), coords(1, 1, 1));
    assert!(far.layers[0].features.is_empty());
    let mut near = tile_of(vec![point]);
    into_descendant(&mut near, coords(0, 0, 0), coords(0, 0, 1));
    assert_eq!(parts(&near), vec![vec![vec![(200, 200)]]]);
    let edge = feature(tile::GeomType::Point, vec![9, zigzag(2112), zigzag(2048)]);
    let mut buffered = tile_of(vec![edge]);
    into_descendant(&mut buffered, coords(0, 0, 0), coords(0, 0, 1));
    assert_eq!(
        parts(&buffered),
        vec![vec![vec![(4224, 4096)]]],
        "a point in the buffer around the tile stays"
    );
}

#[test]
fn a_tile_at_the_source_zoom_is_left_as_it_is() {
    let geometry = vec![9, zigzag(100), zigzag(100)];
    let mut same = tile_of(vec![feature(tile::GeomType::Point, geometry.clone())]);
    into_descendant(&mut same, coords(3, 2, 2), coords(3, 2, 2));
    assert_eq!(same.layers[0].features[0].geometry, geometry);
}

#[test]
fn lines_are_cut_at_the_buffer_and_split_where_they_leave_the_tile() {
    let across = encoded(Kind::Line, &[vec![(0, 2048), (4096, 2048)]]);
    let mut west = tile_of(vec![feature(tile::GeomType::Linestring, across)]);
    into_descendant(&mut west, coords(0, 0, 0), coords(0, 0, 1));
    assert_eq!(parts(&west), vec![vec![vec![(0, 4096), (4224, 4096)]]]);
    // Out through the south edge of the child and back in.
    let dip = encoded(Kind::Line, &[vec![(0, 1024), (1024, 3072), (2048, 1024)]]);
    let mut split = tile_of(vec![feature(tile::GeomType::Linestring, dip)]);
    into_descendant(&mut split, coords(0, 0, 0), coords(0, 0, 1));
    let lines = &parts(&split)[0];
    assert_eq!(lines.len(), 2, "the line leaves the tile and comes back");
    assert_eq!(lines[0][0], (0, 2048));
    assert_eq!(lines[0][1].1, 4224, "cut at the buffer edge");
    assert_eq!(lines[1][1], (4096, 2048));
    readable(&split);
}

#[test]
fn polygons_are_clipped_to_the_tile_and_holes_follow_their_ring() {
    let whole = vec![(0, 0), (4096, 0), (4096, 4096), (0, 4096)];
    let far_hole = vec![(100, 100), (100, 200), (200, 200), (200, 100)];
    let near_hole = vec![(1900, 1900), (1900, 2200), (2200, 2200), (2200, 1900)];
    let polygon = encoded(Kind::Polygon, &[whole, far_hole, near_hole]);
    let mut south_east = tile_of(vec![feature(tile::GeomType::Polygon, polygon)]);
    into_descendant(&mut south_east, coords(0, 0, 0), coords(1, 1, 1));
    let rings = &parts(&south_east)[0];
    assert_eq!(rings.len(), 2, "the far hole is left out");
    assert_eq!(
        rings[0],
        vec![(-128, -128), (4096, -128), (4096, 4096), (-128, 4096)],
        "the exterior ring is cut to the tile and its buffer"
    );
    assert_eq!(
        rings[1],
        vec![(-128, -128), (-128, 304), (304, 304), (304, -128)],
        "the hole over the corner is cut to it"
    );
    readable(&south_east);
    let only_hole = encoded(
        Kind::Polygon,
        &[
            vec![(0, 0), (100, 0), (100, 100), (0, 100)],
            vec![(10, 10), (10, 20), (20, 20), (20, 10)],
        ],
    );
    let mut gone = tile_of(vec![feature(tile::GeomType::Polygon, only_hole)]);
    into_descendant(&mut gone, coords(0, 0, 0), coords(1, 1, 1));
    assert!(
        gone.layers[0].features.is_empty(),
        "a polygon whose exterior ring the tile leaves out goes with its holes"
    );
}

#[test]
fn the_grid_of_a_deep_descendant_is_reached_without_overflow() {
    let point = feature(tile::GeomType::Point, vec![9, zigzag(2048), zigzag(2048)]);
    let mut deep = tile_of(vec![point]);
    let z = 18_u8;
    into_descendant(&mut deep, coords(0, 0, 0), coords(1 << 17, 1 << 17, z));
    assert_eq!(parts(&deep), vec![vec![vec![(0, 0)]]]);
}

#[test]
fn a_larger_extent_keeps_its_own_grid_and_buffer() {
    let mut tile = tile_of(vec![feature(
        tile::GeomType::Point,
        vec![9, zigzag(4224), zigzag(4096)],
    )]);
    tile.layers[0].extent = Some(8192);
    into_descendant(&mut tile, coords(0, 0, 0), coords(0, 0, 1));
    assert_eq!(parts(&tile), vec![vec![vec![(8448, 8192)]]]);
    assert_eq!(tile.layers[0].extent, Some(8192));
}

#[test]
fn command_streams_round_trip_through_the_decoder() {
    let rings = vec![
        vec![(0, 0), (10, 0), (10, 10)],
        vec![(2, 2), (2, 4), (4, 4)],
    ];
    assert_eq!(
        decode(&encoded(Kind::Polygon, &rings)).expect("readable"),
        rings
    );
    let lines = vec![vec![(0, 0), (5, 5)], vec![(7, 7), (9, 9), (9, 0)]];
    assert_eq!(
        decode(&encoded(Kind::Line, &lines)).expect("readable"),
        lines
    );
    let points = vec![vec![(1, 1), (2, 2)]];
    assert_eq!(
        decode(&encoded(Kind::Point, &points)).expect("readable"),
        points
    );
    assert!(
        decode(&[4, 0, 0]).is_none(),
        "an unknown command is refused"
    );
}

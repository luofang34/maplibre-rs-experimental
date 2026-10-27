#![allow(clippy::expect_used, clippy::panic)]
use geozero::mvt::{tile::GeomType, Message, Tile};
use serde_json::json;

use super::super::{GeoJsonIndex, BUFFER, EXTENT};
use crate::{
    coords::{WorldTileCoords, ZoomLevel},
    style::source::GeoJsonSource,
};

fn tile(z: u8, x: i32, y: i32) -> WorldTileCoords {
    WorldTileCoords {
        x,
        y,
        z: ZoomLevel::from(z),
    }
}

fn index(geometry: serde_json::Value) -> GeoJsonIndex {
    let source: GeoJsonSource = serde_json::from_value(json!({"data": geometry})).expect("source");
    GeoJsonIndex::from_value(&geometry, &source).expect("indexes")
}

/// A box from `west`,`south` to `east`,`north` in degrees.
fn rectangle(west: f64, south: f64, east: f64, north: f64) -> serde_json::Value {
    json!({"type": "Polygon", "coordinates": [[
        [west, south], [east, south], [east, north], [west, north], [west, south]]]})
}

/// The rings of the tile's polygon features, in tile units.
fn rings(bytes: &[u8]) -> Vec<Vec<(i32, i32)>> {
    let zigzag = |value: u32| ((value >> 1) as i32) ^ -((value & 1) as i32);
    let layer = Tile::decode(bytes).expect("tile").layers.remove(0);
    let mut rings = Vec::new();
    let mut cursor = (0, 0);
    for feature in &layer.features {
        assert_eq!(feature.r#type, Some(GeomType::Polygon as i32));
        let mut commands = feature.geometry.iter().copied();
        while let Some(command) = commands.next() {
            let (id, count) = (command & 7, command >> 3);
            if id == 1 {
                rings.push(Vec::new());
            }
            for _ in 0..if id == 7 { 0 } else { count } {
                let dx = zigzag(commands.next().expect("dx"));
                let dy = zigzag(commands.next().expect("dy"));
                cursor = (cursor.0 + dx, cursor.1 + dy);
                rings.last_mut().expect("ring").push(cursor);
            }
        }
    }
    rings
}

#[test]
fn a_tile_inside_a_large_polygon_carries_only_its_own_square() {
    let index = index(rectangle(-170.0, -80.0, 170.0, 80.0));
    let rings = rings(&index.tile(tile(4, 5, 6)));
    assert_eq!(rings.len(), 1, "one ring: {rings:?}");
    let edge = BUFFER as i32;
    let far = EXTENT as i32 + edge;
    let mut corners = rings[0].clone();
    corners.sort_unstable();
    assert_eq!(
        corners,
        [(-edge, -edge), (-edge, far), (far, -edge), (far, far)],
        "the ring is the tile and its buffer"
    );
}

#[test]
fn a_cut_polygon_keeps_its_pole_edges_where_the_globe_extends_them() {
    let index = index(rectangle(-170.0, -89.0, 170.0, 89.0));
    let extent = EXTENT as i32;
    let north = rings(&index.tile(tile(2, 1, 0)));
    assert!(
        north[0]
            .windows(2)
            .any(|edge| edge[0].1 == 0 && edge[1].1 == 0),
        "the clamped northern edge lies on the top row's top edge: {north:?}"
    );
    let south = rings(&index.tile(tile(2, 1, 3)));
    assert!(
        south[0]
            .windows(2)
            .any(|edge| edge[0].1 == extent && edge[1].1 == extent),
        "the clamped southern edge lies on the bottom row's bottom edge: {south:?}"
    );
}

#[test]
fn a_polygon_left_outside_the_tile_window_is_not_encoded() {
    // The polygon's bounds reach the tile, its area does not.
    let index = index(json!({"type": "Polygon", "coordinates": [[
        [-170.0, -80.0], [170.0, -80.0], [-170.0, 80.0], [-170.0, -80.0]]]}));
    assert!(rings(&index.tile(tile(3, 7, 1))).is_empty());
}

#[cfg(feature = "headless")]
#[test]
fn a_tile_inside_a_large_polygon_tessellates_to_its_own_area_on_the_globe() {
    let index = index(rectangle(-170.0, -80.0, 170.0, 80.0));
    let style: crate::style::Style = serde_json::from_value(json!({"version": 8,
        "sources": {}, "layers": [{"id": "land", "type": "fill", "source-layer": "_geojson",
            "paint": {"fill-color": "#ff0000"}}]}))
    .expect("style");
    let layers = crate::headless::map::process_tile_layers(
        &index.tile(tile(4, 5, 6)),
        &style.layers[0],
        tile(4, 5, 6),
        crate::projection::ProjectionType::VerticalPerspective,
    )
    .expect("tessellates");
    let indices: usize = layers
        .vector
        .iter()
        .map(|layer| layer.buffer.usable_indices as usize)
        .sum();
    // The buffered tile meets ten subdivision cells per side. Each of the square's two
    // triangles cuts a cell into a polygon of at most five corners, three triangles.
    assert!(
        (1..=10 * 10 * 2 * 3 * 3).contains(&indices),
        "{indices} indices for one tile's square"
    );
}

#![allow(clippy::expect_used)]

use geo_types::{LineString, Point};

use super::Tiles;
use crate::{
    coords::{WorldCoords, WorldTileCoords, Zoom, ZoomLevel, EXTENT, TILE_SIZE},
    tcs::{
        geometry_index::{ExactGeometry, IndexedGeometry, TileIndex},
        tiles::TileComponent,
    },
};

#[derive(Debug, PartialEq)]
struct Height(u32);
impl TileComponent for Height {}

#[test]
fn clearing_tiles_discards_geometry_queries_before_coordinates_are_reused() {
    let mut tiles = Tiles::default();
    let coords = WorldTileCoords::default();
    tiles.spawn_mut(coords).expect("tile").insert(Height(10));
    tiles.geometry_index.index_tile(
        &coords,
        None,
        TileIndex::Linear {
            list: vec![IndexedGeometry {
                bounds: rstar::AABB::from_corners(Point::new(0.0, 0.0), Point::new(EXTENT, EXTENT)),
                exact: ExactGeometry::LineString(LineString::from(vec![
                    (0.0, 0.0),
                    (EXTENT, EXTENT),
                ])),
                properties: std::sync::Arc::new(
                    [(
                        "name".into(),
                        crate::style::expression::Value::String("road".into()),
                    )]
                    .into_iter()
                    .collect(),
                ),
                source_layer: "roads".into(),
                id: None,
                feature_index: 0,
            }],
        },
    );
    let point = WorldCoords::from((TILE_SIZE / 2.0, TILE_SIZE / 2.0));
    let z = ZoomLevel::from(0);
    let zoom = Zoom::new(0.0);
    assert_eq!(
        tiles
            .geometry_index
            .query_point(&point, z, zoom)
            .expect("indexed tile")
            .len(),
        1
    );
    assert!(tiles.geometry_index.approximate_bytes() > 0);
    tiles.clear();
    assert!(!tiles.exists(coords));
    assert!(tiles.query::<&Height>(coords).is_none());
    assert!(tiles.geometry_index.query_point(&point, z, zoom).is_none());
    assert_eq!(tiles.geometry_index.approximate_bytes(), 0);
    tiles
        .spawn_mut(coords)
        .expect("reused coordinates")
        .insert(Height(30));
    assert!(tiles.geometry_index.query_point(&point, z, zoom).is_none());
}

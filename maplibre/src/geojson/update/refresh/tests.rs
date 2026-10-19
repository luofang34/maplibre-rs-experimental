#![allow(clippy::expect_used)]
use std::sync::Arc;

use serde_json::{json, Value};

use super::GeoJsonVersions;
use crate::{
    coords::WorldTileCoords,
    geojson::update::GeoJsonDiff,
    io::tile_retry::{self, RequestDisposition, RequestKind, TileRequestOutcome},
    style::{source::GeoJsonData, Style},
    tcs::world::World,
};

fn style(source: Value) -> Style {
    serde_json::from_value(json!({"version": 8, "sources": {"route": source}, "layers": []}))
        .expect("style")
}

fn inline(features: Value) -> Value {
    json!({"type": "geojson", "data": collection(features)})
}

fn collection(features: Value) -> Value {
    json!({"type": "FeatureCollection", "features": features})
}

fn line(id: u64, from: [f64; 2], to: [f64; 2]) -> Value {
    json!({"type": "Feature", "id": id, "properties": {},
        "geometry": {"type": "LineString", "coordinates": [from, to]}})
}

fn point(id: u64, at: [f64; 2]) -> Value {
    json!({"type": "Feature", "id": id, "properties": {},
        "geometry": {"type": "Point", "coordinates": at}})
}

fn tile(x: i32, y: i32, z: u8) -> WorldTileCoords {
    WorldTileCoords::from((x, y, z.into()))
}

/// A world whose `tiles` were requested and have loaded, with no retry due.
fn loaded(tiles: &[WorldTileCoords]) -> World {
    let mut world = World::default();
    for (attempt, coords) in (1..).zip(tiles) {
        tile_retry::started(&mut world, *coords, RequestKind::Vector, attempt);
        tile_retry::completed(
            &mut world,
            TileRequestOutcome {
                coords: *coords,
                kind: RequestKind::Vector,
                attempt: Some(attempt),
                disposition: RequestDisposition::Complete,
            },
        );
    }
    world
}

/// The tiles the request system will fetch again.
fn refreshed(world: &World, tiles: &[WorldTileCoords]) -> Vec<WorldTileCoords> {
    tiles
        .iter()
        .copied()
        .filter(|coords| tile_retry::waiting(world, *coords, RequestKind::Vector))
        .collect()
}

fn set(style: &mut Style, world: &mut World, data: Value) {
    let before = GeoJsonVersions::of(style);
    style
        .set_geojson_data("route", GeoJsonData::Inline(Arc::new(data)))
        .expect("set data");
    before.refresh_changed(style, world);
}

// At zoom 2 a tile spans 90 degrees of longitude; rows meet at the equator.
const EAST_SOUTH: (i32, i32) = (2, 2);
const WEST_NORTH: (i32, i32) = (0, 1);

#[test]
fn moving_one_feature_refreshes_only_the_tiles_it_was_and_is_in() {
    let route = line(1, [10.0, -10.0], [20.0, -20.0]);
    let parked = point(2, [-150.0, 40.0]);
    let mut style = style(inline(json!([route, parked])));
    let near = tile(EAST_SOUTH.0, EAST_SOUTH.1, 2);
    let far = tile(WEST_NORTH.0, WEST_NORTH.1, 2);
    let (overview, deeper) = (tile(0, 0, 0), tile(17, 17, 5));
    let tiles = [near, far, overview, deeper];
    let mut world = loaded(&tiles);
    assert!(refreshed(&world, &tiles).is_empty());

    let moved = line(1, [12.0, -10.0], [20.0, -20.0]);
    set(&mut style, &mut world, collection(json!([moved, parked])));
    assert_eq!(
        refreshed(&world, &tiles),
        [near, overview, deeper],
        "every zoom's tiles under the route refresh, the far one keeps its unchanged point"
    );
}

#[test]
fn replacing_data_with_the_same_features_refreshes_nothing() {
    let features = json!([
        line(1, [10.0, -10.0], [20.0, -20.0]),
        point(2, [-150.0, 40.0])
    ]);
    let mut style = style(inline(features.clone()));
    let tiles = [tile(0, 0, 0), tile(2, 2, 2), tile(0, 1, 2)];
    let mut world = loaded(&tiles);
    set(&mut style, &mut world, collection(features));
    assert!(refreshed(&world, &tiles).is_empty());
}

#[test]
fn data_behind_a_url_or_in_clusters_refreshes_every_loaded_tile() {
    let tiles = [tile(2, 2, 2), tile(0, 1, 2)];
    let mut style = style(json!({"type": "geojson", "data": "https://example.com/a.geojson"}));
    let mut world = loaded(&tiles);
    let before = GeoJsonVersions::of(&style);
    style
        .set_geojson_data(
            "route",
            GeoJsonData::Url("https://example.com/b.geojson".into()),
        )
        .expect("set url");
    before.refresh_changed(&style, &mut world);
    assert_eq!(
        refreshed(&world, &tiles),
        tiles,
        "a URL's features are unknown"
    );

    let mut clustered = style_with_cluster(json!([point(1, [10.0, -10.0])]));
    let mut world = loaded(&tiles);
    set(
        &mut clustered,
        &mut world,
        collection(json!([point(1, [11.0, -10.0])])),
    );
    assert_eq!(
        refreshed(&world, &tiles),
        tiles,
        "a moved point can regroup clusters anywhere"
    );
}

fn style_with_cluster(features: Value) -> Style {
    style(json!({"type": "geojson", "cluster": true, "data": collection(features)}))
}

#[test]
fn a_feature_just_outside_a_tile_refreshes_it_through_the_index_buffer() {
    // Column 1 begins at -90 degrees; the buffer reaches 128 / 4096 of a tile, 2.8 degrees, past it.
    let column_0 = tile(0, 2, 2);
    let column_1 = tile(1, 2, 2);
    let tiles = [column_0, column_1];

    let mut style = style(inline(json!([point(1, [-92.0, -10.0])])));
    let mut world = loaded(&tiles);
    set(
        &mut style,
        &mut world,
        collection(json!([point(1, [-92.5, -10.0])])),
    );
    assert_eq!(refreshed(&world, &tiles), tiles, "inside the buffer");

    let mut style = self::style(inline(json!([point(1, [-95.0, -10.0])])));
    let mut world = loaded(&tiles);
    set(
        &mut style,
        &mut world,
        collection(json!([point(1, [-95.5, -10.0])])),
    );
    assert_eq!(refreshed(&world, &tiles), [column_0], "beyond the buffer");
}

#[test]
fn removing_a_feature_by_id_leaves_the_tiles_of_the_features_after_it() {
    let (west, east) = (tile(0, 1, 2), tile(2, 2, 2));
    let tiles = [west, east];
    let mut style = style(inline(json!([
        point(1, [-150.0, 40.0]),
        point(2, [10.0, -10.0])
    ])));
    let mut world = loaded(&tiles);
    let before = GeoJsonVersions::of(&style);
    let diff: GeoJsonDiff = serde_json::from_value(json!({"remove": [1]})).expect("diff");
    style
        .update_geojson_data("route", &diff)
        .expect("remove by id");
    before.refresh_changed(&style, &mut world);
    assert_eq!(refreshed(&world, &tiles), [west]);
}

#[test]
fn generated_ids_follow_position_so_a_removal_reaches_the_features_after_it() {
    let (west, east) = (tile(0, 1, 2), tile(2, 2, 2));
    let tiles = [west, east];
    let anonymous = |at: [f64; 2]| {
        json!({"type": "Feature", "properties": {},
            "geometry": {"type": "Point", "coordinates": at}})
    };
    let mut style = style(json!({"type": "geojson", "generateId": true,
        "data": collection(json!([anonymous([-150.0, 40.0]), anonymous([10.0, -10.0])]))}));
    let mut world = loaded(&tiles);
    set(
        &mut style,
        &mut world,
        collection(json!([anonymous([10.0, -10.0])])),
    );
    assert_eq!(refreshed(&world, &tiles), tiles);
}

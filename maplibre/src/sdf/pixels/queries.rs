//! Rendered symbol queries against a placed, drawn label.
use serde_json::json;

use super::{fixture_map, layers, style};
use crate::sdf::query::{query_rendered_symbols_in, QueryError, QueryGeometry, QueryOptions};

const AT_LABEL: [f64; 2] = [256.0, 256.0];

fn options(layers: Option<&[&str]>, filter: Option<serde_json::Value>) -> QueryOptions {
    QueryOptions {
        layers: layers.map(|layers| layers.iter().map(|layer| (*layer).to_owned()).collect()),
        filter,
    }
}

async fn label_map() -> crate::headless::map::HeadlessMap {
    let style = style(0.0, "ground");
    let layers = layers(&style);
    fixture_map(style, layers, 1).await
}

#[tokio::test]
async fn a_box_finds_the_label_it_overlaps_and_a_distant_box_finds_nothing() {
    let map = label_map().await;
    let near = QueryGeometry::Box {
        min: [AT_LABEL[0] - 4.0, AT_LABEL[1] - 4.0],
        max: [AT_LABEL[0] + 4.0, AT_LABEL[1] + 4.0],
    };
    let found = map
        .query_rendered_symbols_in(near, &QueryOptions::default())
        .expect("query");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].text, "Alps");
    let far = QueryGeometry::Box {
        min: [0.0, 0.0],
        max: [20.0, 20.0],
    };
    assert!(map
        .query_rendered_symbols_in(far, &QueryOptions::default())
        .expect("query")
        .is_empty());
    let reversed = QueryGeometry::Box {
        min: near_max(near),
        max: near_min(near),
    };
    assert_eq!(
        map.query_rendered_symbols_in(reversed, &QueryOptions::default())
            .expect("query")
            .len(),
        1,
        "opposite corners may be given in either order"
    );
}

fn near_min(geometry: QueryGeometry) -> [f64; 2] {
    match geometry {
        QueryGeometry::Box { min, .. } => min,
        QueryGeometry::Point(point) => point,
    }
}

fn near_max(geometry: QueryGeometry) -> [f64; 2] {
    match geometry {
        QueryGeometry::Box { max, .. } => max,
        QueryGeometry::Point(point) => point,
    }
}

#[tokio::test]
async fn a_point_query_matches_the_older_entry_point() {
    let map = label_map().await;
    let point = map
        .query_rendered_symbols_in(QueryGeometry::Point(AT_LABEL), &QueryOptions::default())
        .expect("query");
    assert_eq!(point.len(), 1);
    assert_eq!(
        point.len(),
        map.query_rendered_symbols(AT_LABEL, None).len()
    );
}

#[tokio::test]
async fn layer_lists_are_validated_and_restrict_the_result() {
    let map = label_map().await;
    let at = QueryGeometry::Point(AT_LABEL);
    assert_eq!(
        map.query_rendered_symbols_in(at, &options(Some(&["label"]), None))
            .expect("query")
            .len(),
        1
    );
    assert!(map
        .query_rendered_symbols_in(at, &options(Some(&["point"]), None))
        .expect("query")
        .is_empty());
    assert_eq!(
        map.query_rendered_symbols_in(at, &options(Some(&["nowhere"]), None))
            .err(),
        Some(QueryError::UnknownLayer {
            layer: "nowhere".into()
        })
    );
}

#[tokio::test]
async fn filters_see_the_geometry_type_and_properties_of_each_symbol() {
    let map = label_map().await;
    let at = QueryGeometry::Point(AT_LABEL);
    let with = |filter| map.query_rendered_symbols_in(at, &options(None, Some(filter)));
    assert_eq!(
        with(json!(["==", "$type", "Point"])).expect("query").len(),
        1
    );
    assert!(with(json!(["==", "$type", "Polygon"]))
        .expect("query")
        .is_empty());
    assert!(
        with(json!(["==", ["get", "missing"], 1]))
            .expect("query")
            .is_empty(),
        "a property the symbol lacks fails the comparison"
    );
    assert!(matches!(with(json!(7)), Err(QueryError::InvalidFilter(_))));
}

#[tokio::test]
async fn invalid_geometry_is_rejected_and_hidden_layers_are_skipped() {
    let map = label_map().await;
    assert_eq!(
        map.query_rendered_symbols_in(
            QueryGeometry::Point([f64::NAN, 0.0]),
            &QueryOptions::default()
        )
        .err(),
        Some(QueryError::InvalidGeometry)
    );
    let mut hidden = style(0.0, "ground");
    hidden.layers[2].visibility = crate::style::layer::LayerVisibility::None;
    let found = query_rendered_symbols_in(
        map.world(),
        &hidden,
        QueryGeometry::Point(AT_LABEL),
        &QueryOptions::default(),
    )
    .expect("query");
    assert!(found.is_empty(), "a layer switched off is not queryable");
}

#[tokio::test]
async fn a_pan_moves_the_placed_label_within_the_next_frame() {
    let mut map = label_map().await;
    let camera = map.view_state().camera().position();
    map.view_state_mut()
        .camera_mut()
        .move_to(cgmath::Point2::new(camera.x + 40.0, camera.y));
    map.run_frame().expect("frame after the pan");
    let old_place = QueryGeometry::Box {
        min: [AT_LABEL[0] - 4.0, AT_LABEL[1] - 4.0],
        max: [AT_LABEL[0] + 4.0, AT_LABEL[1] + 4.0],
    };
    assert!(
        map.query_rendered_symbols_in(old_place, &QueryOptions::default())
            .expect("query")
            .is_empty(),
        "placement still holds the label where the camera left it"
    );
    let screen = QueryGeometry::Box {
        min: [0.0, 0.0],
        max: [512.0, 512.0],
    };
    assert_eq!(
        map.query_rendered_symbols_in(screen, &QueryOptions::default())
            .expect("query")
            .len(),
        1
    );
}

#[tokio::test]
async fn only_the_first_eye_of_a_frame_places_symbols() {
    use crate::render::eye_covering::EyeInFrame;

    let mut map = label_map().await;
    let camera = map.view_state().camera().position();
    map.view_state_mut()
        .camera_mut()
        .move_to(cgmath::Point2::new(camera.x + 40.0, camera.y));
    let old_place = QueryGeometry::Box {
        min: [AT_LABEL[0] - 4.0, AT_LABEL[1] - 4.0],
        max: [AT_LABEL[0] + 4.0, AT_LABEL[1] + 4.0],
    };
    let hits = |map: &crate::headless::map::HeadlessMap| {
        map.query_rendered_symbols_in(old_place, &QueryOptions::default())
            .expect("query")
            .len()
    };
    map.world_mut()
        .resources
        .insert(EyeInFrame { index: 1, frame: 1 });
    map.run_frame().expect("second eye");
    assert_eq!(
        hits(&map),
        1,
        "the second eye reuses the first eye's placement"
    );
    map.world_mut()
        .resources
        .insert(EyeInFrame { index: 0, frame: 2 });
    map.run_frame().expect("first eye of the next frame");
    assert_eq!(hits(&map), 0, "the first eye places for the moved view");
}

#[tokio::test]
async fn a_label_buried_in_terrain_is_neither_placed_nor_queryable() {
    let style = style(0.0, "absolute");
    let layers = layers(&style);
    let map = fixture_map(style, layers, 1).await;
    let screen = QueryGeometry::Box {
        min: [0.0, 0.0],
        max: [512.0, 512.0],
    };
    assert!(
        map.query_rendered_symbols_in(screen, &QueryOptions::default())
            .expect("query")
            .is_empty(),
        "the sea-level label sits under 1200 m of ground and is not drawn"
    );
}

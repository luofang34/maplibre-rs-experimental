use serde_json::json;

use super::{center_rgba, tile, vector_map, world_box_feature};
use crate::{
    background::BackgroundPlugin,
    headless::{
        create_headless_renderer,
        map::{process_geojson_layers, HeadlessMap},
        HeadlessPlugin,
    },
    io::tile_retry::{self, RequestDisposition, RequestKind, TileRequestOutcome},
    projection::ProjectionType,
    render::{eventually::Eventually, RenderPlugin},
    sdf::query::{QueryGeometry, QueryOptions},
    style::{feature_state::FeatureStateTarget, Style},
    vector::{
        feature_paint::TileFeaturePaintComponent, DefaultVectorTransferables, VectorBufferPool,
        VectorPlugin,
    },
};

#[tokio::test]
async fn late_arriving_processed_tiles_pick_up_live_feature_state_and_paint() {
    let style: Style = serde_json::from_value(json!({
        "version": 8,
        "center": [0.0, 0.0],
        "zoom": 0.0,
        "sources": {
            "parks": {
                "type": "geojson",
                "promoteId": "slug",
                "data": {
                    "type": "FeatureCollection",
                    "features": [world_box_feature(json!({"slug": "central", "title": "Central"}))]
                }
            }
        },
        "layers": [
            {"id": "bg", "type": "background", "paint": {"background-color": "#000000"}},
            {
                "id": "park-fill",
                "type": "fill",
                "source": "parks",
                "paint": {
                    "fill-color": ["case", ["boolean", ["feature-state", "hover"], false], "#00ff00", "#ff0000"]
                }
            }
        ]
    }))
    .expect("valid style");

    let raw_geojson = json!({
        "type": "FeatureCollection",
        "features": [world_box_feature(json!({"slug": "central", "title": "Central"}))]
    });
    let stale_annotated = style.apply_feature_state_to_geojson("parks", &raw_geojson);
    let layers = style
        .layers
        .iter()
        .filter(|layer| layer.source.as_deref() == Some("parks"))
        .cloned()
        .collect();
    let stale_processed = process_geojson_layers(
        &stale_annotated,
        "parks",
        layers,
        tile(),
        ProjectionType::Mercator,
    )
    .expect("stale worker output before feature-state");

    let (kernel, renderer) = create_headless_renderer(64, 64, None)
        .await
        .expect("headless renderer");
    let mut map = HeadlessMap::new(
        style,
        renderer,
        kernel,
        vec![
            Box::new(RenderPlugin),
            Box::new(BackgroundPlugin),
            Box::new(VectorPlugin::<DefaultVectorTransferables>::default()),
            Box::new(HeadlessPlugin::new(false).preserve_tile_sources()),
        ],
    )
    .expect("headless map");

    let target = FeatureStateTarget::geojson("parks");
    let mut hover_state = serde_json::Map::new();
    hover_state.insert("hover".to_string(), json!(true));
    map.set_feature_state(target, "central", hover_state);

    map.render_frames_with_terrain(stale_processed, vec![], vec![], 2)
        .expect("deliver late worker output");
    assert_eq!(
        center_rgba(&map),
        [0, 255, 0, 255],
        "late-arriving worker output must synchronize against live feature-state on arrival"
    );

    let queried = map
        .query_rendered_features(
            QueryGeometry::Point([32.0, 32.0]),
            &QueryOptions {
                layers: Some(vec!["park-fill".to_string()]),
                filter: None,
            },
        )
        .expect("query late-arrived feature");
    assert_eq!(queried.len(), 1);
    assert_eq!(queried[0].state.get("hover"), Some(&json!(true)));
}

#[tokio::test]
async fn feature_state_only_schedules_worker_retry_when_dependent_paint_store_is_missing() {
    let mut map = vector_map(json!({
        "version": 8,
        "center": [0.0, 0.0],
        "zoom": 0.0,
        "sources": {
            "parks": {
                "type": "geojson",
                "promoteId": "slug",
                "data": {
                    "type": "FeatureCollection",
                    "features": [world_box_feature(json!({"slug": "central"}))]
                }
            }
        },
        "layers": [
            {"id": "bg", "type": "background", "paint": {"background-color": "#000000"}},
            {"id": "park-fill", "type": "fill", "source": "parks", "paint": {"fill-color": "#ff0000"}}
        ]
    }))
    .await;

    let attempt = tile_retry::next_attempt(&mut map.map_context.world);
    tile_retry::started(
        &mut map.map_context.world,
        tile(),
        RequestKind::Vector,
        attempt,
        None,
    );
    tile_retry::completed(
        &mut map.map_context.world,
        TileRequestOutcome {
            coords: tile(),
            kind: RequestKind::Vector,
            attempt: Some(attempt),
            disposition: RequestDisposition::Complete,
        },
    );
    let keeps_a_feature = map
        .world()
        .tiles
        .query::<&TileFeaturePaintComponent>(tile())
        .is_some_and(|store| {
            store
                .committed
                .values()
                .chain(store.pending.values())
                .flatten()
                .any(|entry| entry.raw_id.is_some() || !entry.properties.is_empty())
        });
    assert!(
        !keeps_a_feature,
        "constant-paint layers keep slot kinds at most, never a feature's id or properties"
    );

    let target = FeatureStateTarget::geojson("parks");
    let mut hover = serde_json::Map::new();
    hover.insert("hover".to_string(), json!(true));
    map.set_feature_state(target, "central", hover);
    map.run_frame().expect("frame");
    assert!(
        !tile_retry::waiting(map.world(), tile(), RequestKind::Vector),
        "layers that do not read feature-state must not trigger vector tile re-requests"
    );
}

#[tokio::test]
async fn editing_one_style_layer_leaves_other_vector_entries_revisions_untouched() {
    let mut map = vector_map(json!({
        "version": 8,
        "center": [0.0, 0.0],
        "zoom": 0.0,
        "sources": {
            "parks": {
                "type": "geojson",
                "data": {
                    "type": "FeatureCollection",
                    "features": [world_box_feature(json!({"kind": "park"}))]
                }
            }
        },
        "layers": [
            {"id": "bg", "type": "background", "paint": {"background-color": "#000000"}},
            {"id": "bottom", "type": "fill", "source": "parks", "paint": {"fill-color": "#ff0000"}},
            {"id": "top", "type": "fill", "source": "parks", "paint": {"fill-color": "#00ff00"}}
        ]
    }))
    .await;

    let entry_rev = |map: &HeadlessMap, id: &str| -> u64 {
        let Some(Eventually::Initialized(pool)) =
            map.world().resources.get::<Eventually<VectorBufferPool>>()
        else {
            panic!("initialized vector pool");
        };
        pool.index()
            .get_layers(tile())
            .expect("tile entries")
            .iter()
            .find(|entry| entry.style_layer.id == id)
            .expect("layer entry")
            .content_revision()
    };

    let bottom_before = entry_rev(&map, "bottom");
    let top_before = entry_rev(&map, "top");

    map.mutate_style(|style| style.set_paint_property("top", "fill-color", json!("#0000ff")))
        .expect("edit top paint");
    map.run_frame().expect("upload paint edit");

    assert_eq!(
        entry_rev(&map, "bottom"),
        bottom_before,
        "untouched style layers must keep their content_revision so terrain drapes are not redrawn"
    );
    assert_ne!(
        entry_rev(&map, "top"),
        top_before,
        "edited style layer must bump its content_revision"
    );
}

#[tokio::test]
async fn recolouring_a_constant_fill_beside_its_outline_keeps_the_tile() {
    let mut map = vector_map(json!({
        "version": 8,
        "center": [0.0, 0.0],
        "zoom": 0.0,
        "sources": {
            "parks": {
                "type": "geojson",
                "data": {
                    "type": "FeatureCollection",
                    "features": [world_box_feature(json!({"kind": "park"}))]
                }
            }
        },
        "layers": [
            {"id": "bg", "type": "background", "paint": {"background-color": "#000000"}},
            {"id": "park-fill", "type": "fill", "source": "parks",
             "paint": {"fill-color": "#ff0000", "fill-outline-color": "#0000ff"}}
        ]
    }))
    .await;
    assert_eq!(center_rgba(&map), [255, 0, 0, 255]);

    let change = map
        .mutate_style(|style| style.set_paint_property("park-fill", "fill-color", json!("#00ff00")))
        .expect("constant paint update");
    assert!(
        !change.redraw_tiles,
        "a constant fill colour is rewritten in place"
    );
    map.run_frame().expect("in-place paint frame");
    assert!(
        !tile_retry::waiting(map.world(), tile(), RequestKind::Vector),
        "the fill and its outline are told apart without asking a worker for the tile again"
    );
    assert_eq!(center_rgba(&map), [0, 255, 0, 255]);
}

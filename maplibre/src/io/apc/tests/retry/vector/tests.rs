use super::super::{
    fixture::{Fixture, Kind},
    source::Response,
};

#[tokio::test]
async fn stationary_vector_tile_recovers_after_real_http_failure() {
    let mut test = Fixture::new(Kind::Vector, false).await;
    let mut frames = super::render::Frames::new(&mut test);
    test.frame(0);
    test.receive().await;
    assert_eq!(test.source.requests(), 1);
    assert!(!test.loaded());
    test.source.set(Response::Bytes(super::tile()));
    test.frame(999);
    test.receive().await;
    assert_eq!(test.source.requests(), 1, "failure backs off");
    test.frame(1000);
    test.frame(1001);
    assert_eq!(
        test.kernel.apc().pending(),
        1,
        "stationary view admits one retry at its deadline"
    );
    test.receive().await;
    assert_eq!(test.source.requests(), 2);
    assert!(
        test.loaded(),
        "valid MVT reaches the existing tile component"
    );
    super::render::assert_green(&frames.render(&mut test));
    test.frame(60000);
    test.receive().await;
    assert_eq!(test.source.requests(), 2, "success clears the deadline");
}

#[tokio::test]
async fn evicted_vector_completions_cannot_finish_the_new_request() {
    use crate::io::tile_backpressure::{request_budget, MAX_TILES_IN_FLIGHT};
    let mut test = Fixture::new(Kind::Vector, false).await;
    test.frame(0);
    test.kernel.apc().complete().await;
    let old = test.kernel.apc().take_replies();
    test.context.world.tiles.remove(Default::default());
    test.frame(1);
    assert_eq!(test.kernel.apc().pending(), 1);
    test.kernel.apc().deliver(old);
    test.populate.run(&mut test.context).expect("old replies");
    assert_eq!(
        request_budget(&test.context.world),
        MAX_TILES_IN_FLIGHT - 1,
        "late TileTessellated and final outcome cannot finish the new attempt"
    );
    test.frame(60000);
    assert_eq!(test.kernel.apc().pending(), 1, "no duplicate request");
    test.source.set(Response::Bytes(super::tile()));
    test.receive().await;
    assert!(test.loaded());
}

#[tokio::test]
async fn repeated_success_keeps_one_current_bucket_per_style_layer() {
    use crate::{
        io::apc::{AsyncProcedureCall, Input},
        sdf::SymbolLayersDataComponent,
        vector::{DefaultVectorTransferables, VectorLayerBucketComponent},
    };
    let mut test = Fixture::new(Kind::Vector, false).await;
    test.source.set(Response::Bytes(super::tile()));
    test.context
        .world
        .tiles
        .spawn_mut(Default::default())
        .expect("tile")
        .insert(VectorLayerBucketComponent::default())
        .insert(SymbolLayersDataComponent::default());
    for _ in 0..2 {
        test.kernel
            .apc()
            .call(
                Input::TileRequest {
                    coords: Default::default(),
                    style: test.context.style.clone(),
                },
                crate::vector::request_system::fetch_vector_apc::<_, DefaultVectorTransferables, _>,
            )
            .expect("admitted direct worker");
        test.receive().await;
    }
    assert_eq!(test.source.requests(), 2);
    let component = test
        .context
        .world
        .tiles
        .query::<&VectorLayerBucketComponent>(Default::default())
        .expect("existing component");
    assert_eq!(
        component.layers.len(),
        1,
        "style layer content replaces its own bucket"
    );
}

#[tokio::test]
async fn partial_vector_sources_remain_incomplete_until_the_matching_final_reply() {
    use crate::{
        io::tile_backpressure::{request_budget, MAX_TILES_IN_FLIGHT},
        vector::VectorLayerBucketComponent,
    };
    let mut test = Fixture::new(Kind::Vector, true).await;
    let mut frames = super::render::Frames::new(&mut test);
    test.source.set_healthy(Response::Bytes(super::tile()));
    test.frame(0);
    test.receive().await;
    assert_eq!(test.source.requests(), 2);
    assert!(
        !test.loaded(),
        "one healthy source does not complete both sources"
    );
    test.source.set(Response::Bytes(super::tile()));
    let gate = test.source.block_unstable();
    test.frame(1000);
    let kernel = test.kernel.clone();
    let work = kernel.apc().complete();
    let observer = async {
        gate.entered.notified().await;
        test.populate
            .run(&mut test.context)
            .expect("partial result");
        assert!(
            !test.loaded(),
            "first source cannot clear the prior missing source"
        );
        assert_eq!(request_budget(&test.context.world), MAX_TILES_IN_FLIGHT - 1);
        gate.release.notify_one();
    };
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::join!(work, observer);
    })
    .await
    .expect("worker completes");
    test.populate.run(&mut test.context).expect("final result");
    assert!(test.loaded());
    assert_eq!(request_budget(&test.context.world), MAX_TILES_IN_FLIGHT);
    assert_eq!(
        test.context
            .world
            .tiles
            .query::<&VectorLayerBucketComponent>(Default::default())
            .expect("tile")
            .layers
            .len(),
        2
    );
    super::render::assert_green(&frames.render(&mut test));
}

fn labelled_point_tile() -> Vec<u8> {
    use geozero::mvt::{tile, Message, Tile};
    Tile {
        layers: vec![tile::Layer {
            version: 2,
            name: "land".into(),
            features: vec![tile::Feature {
                r#type: Some(tile::GeomType::Point as i32),
                geometry: vec![9, 4096, 4096],
                ..Default::default()
            }],
            extent: Some(8192),
            ..Default::default()
        }],
    }
    .encode_to_vec()
}

fn tile_is_complete(test: &Fixture) -> bool {
    test.context
        .world
        .tiles
        .query::<&crate::vector::VectorLayerBucketComponent>(Default::default())
        .is_some_and(|component| component.done && !component.failed)
}

#[tokio::test]
async fn transient_glyph_failure_retries_the_tile_until_symbols_load() {
    let mut test = Fixture::new(Kind::Vector, false).await;
    let url = test.source.url.clone();
    test.context.style = serde_json::from_value(serde_json::json!({"version":8,
        "sources":{"source":{"type":"vector","tiles":[format!("{url}/unstable/{{z}}/{{x}}/{{y}}")],"maxzoom":0}},
        "glyphs":format!("{url}/healthy/glyphs/{{fontstack}}/{{range}}.pbf"),
        "layers":[
            {"id":"land","source":"source","source-layer":"land","type":"fill","paint":{"fill-color":"#00ff00"}},
            {"id":"label","source":"source","source-layer":"land","type":"symbol",
             "layout":{"text-field":"A","text-font":["Font A"]}}]}))
    .expect("style with symbols");
    test.source.set(Response::Bytes(labelled_point_tile()));
    test.source.set_healthy(Response::Status(503));
    test.frame(0);
    test.receive().await;
    assert_eq!(test.source.requests(), 2, "tile and one glyph range");
    assert!(
        tile_is_complete(&test),
        "the tile stays complete, drawn without labels, while symbol assets are retried"
    );
    test.source.set_healthy(Response::Bytes(
        include_bytes!("../../../../../../../data/0-255.pbf").to_vec(),
    ));
    test.frame(999);
    test.receive().await;
    assert_eq!(test.source.requests(), 2, "the failure backs off");
    test.frame(1000);
    test.frame(1001);
    assert_eq!(test.kernel.apc().pending(), 1, "one retry at the deadline");
    test.receive().await;
    assert_eq!(
        test.source.requests(),
        4,
        "the retry refetches tile and glyphs"
    );
    test.frame(60000);
    test.receive().await;
    assert_eq!(
        test.source.requests(),
        4,
        "the recovered tile schedules no more retries"
    );
}

fn geojson_style(data: serde_json::Value) -> crate::style::Style {
    serde_json::from_value(serde_json::json!({"version":8,
        "sources":{"shapes":{"type":"geojson","data":data}},
        "layers":[{"id":"area","source":"shapes","type":"fill","paint":{"fill-color":"#00ff00"}}]}))
    .expect("geojson style")
}

const WORLD_POLYGON: &str = r#"{"type":"Feature","properties":{},"geometry":{"type":"Polygon",
    "coordinates":[[[-100,-60],[100,-60],[100,60],[-100,60],[-100,-60]]]}}"#;

#[tokio::test]
async fn inline_geojson_declared_in_the_style_reaches_the_screen_without_any_request() {
    let mut test = Fixture::new(Kind::Vector, false).await;
    test.context.style = geojson_style(serde_json::from_str(WORLD_POLYGON).expect("polygon"));
    let mut frames = super::render::Frames::new(&mut test);
    test.frame(0);
    test.receive().await;
    assert_eq!(test.source.requests(), 0, "inline data is never fetched");
    assert!(
        test.loaded(),
        "the polygon is tiled, tessellated and stored"
    );
    super::render::assert_green(&frames.render(&mut test));
}

#[tokio::test]
async fn url_geojson_retries_a_transient_failure_and_then_renders() {
    let mut test = Fixture::new(Kind::Vector, false).await;
    let url = format!("{}/unstable/shapes.geojson", test.source.url);
    test.context.style = geojson_style(serde_json::Value::String(url));
    let mut frames = super::render::Frames::new(&mut test);
    test.frame(0);
    test.receive().await;
    assert_eq!(test.source.requests(), 1);
    assert!(!test.loaded(), "a 503 leaves the tile without data");
    test.source
        .set(Response::Bytes(WORLD_POLYGON.as_bytes().to_vec()));
    test.frame(999);
    test.receive().await;
    assert_eq!(test.source.requests(), 1, "the failure backs off");
    test.frame(1000);
    test.frame(1001);
    assert_eq!(test.kernel.apc().pending(), 1, "one retry at the deadline");
    test.receive().await;
    assert_eq!(test.source.requests(), 2);
    assert!(test.loaded());
    super::render::assert_green(&frames.render(&mut test));
    test.frame(60000);
    test.receive().await;
    assert_eq!(test.source.requests(), 2, "success clears the deadline");
}

#[tokio::test]
async fn a_malformed_geojson_document_is_a_terminal_typed_failure_that_is_not_retried() {
    let mut test = Fixture::new(Kind::Vector, false).await;
    let url = format!("{}/unstable/shapes.geojson", test.source.url);
    test.context.style = geojson_style(serde_json::Value::String(url));
    test.source
        .set(Response::Bytes(b"<html>not geojson".to_vec()));
    test.frame(0);
    test.receive().await;
    assert_eq!(test.source.requests(), 1);
    assert!(!test.loaded());
    test.frame(60000);
    test.receive().await;
    assert_eq!(
        test.source.requests(),
        1,
        "a bad document is remembered, not refetched"
    );
}

fn inline(data: &str) -> crate::style::source::GeoJsonData {
    crate::style::source::GeoJsonData::Inline(std::sync::Arc::new(
        serde_json::from_str(data).expect("geojson"),
    ))
}

const NOTHING: &str = r#"{"type":"FeatureCollection","features":[]}"#;

#[tokio::test]
async fn set_data_replaces_loaded_tiles_and_an_empty_source_clears_them() {
    let mut test = Fixture::new(Kind::Vector, false).await;
    test.context.style = geojson_style(serde_json::from_str(WORLD_POLYGON).expect("polygon"));
    let mut frames = super::render::Frames::new(&mut test);
    test.frame(0);
    test.receive().await;
    assert!(test.loaded());
    test.context
        .set_geojson_data("shapes", inline(NOTHING))
        .expect("set data");
    test.frame(1);
    assert_eq!(
        test.kernel.apc().pending(),
        1,
        "the loaded tile is requested again"
    );
    assert!(
        test.loaded(),
        "the old data stays until the new tile arrives"
    );
    test.receive().await;
    assert!(!test.loaded(), "an empty source leaves nothing to draw");
    test.context
        .set_geojson_data("shapes", inline(WORLD_POLYGON))
        .expect("set data again");
    test.frame(2);
    test.receive().await;
    assert!(test.loaded());
    super::render::assert_green(&frames.render(&mut test));
    assert_eq!(
        test.source.requests(),
        0,
        "inline data never leaves the process"
    );
}

#[tokio::test]
async fn a_reply_to_an_older_generation_cannot_stay_after_set_data() {
    let mut test = Fixture::new(Kind::Vector, false).await;
    test.context.style = geojson_style(serde_json::from_str(WORLD_POLYGON).expect("polygon"));
    test.frame(0);
    assert_eq!(test.kernel.apc().pending(), 1, "first load in flight");
    test.context
        .set_geojson_data("shapes", inline(NOTHING))
        .expect("set data during the load");
    test.receive().await;
    assert!(
        test.loaded(),
        "the reply to the old generation is shown briefly"
    );
    test.frame(1);
    assert_eq!(
        test.kernel.apc().pending(),
        1,
        "the newer generation is requested"
    );
    test.receive().await;
    assert!(!test.loaded(), "and the newer data wins");
}

#[tokio::test]
async fn edits_by_feature_reach_loaded_tiles() {
    let mut test = Fixture::new(Kind::Vector, false).await;
    test.context.style = geojson_style(serde_json::json!({"type":"FeatureCollection",
        "features":[{"type":"Feature","id":1,"properties":{},"geometry":
            serde_json::from_str::<serde_json::Value>(WORLD_POLYGON).expect("polygon")["geometry"]}]}));
    test.frame(0);
    test.receive().await;
    assert!(test.loaded());
    let removal: crate::geojson::update::GeoJsonDiff =
        serde_json::from_value(serde_json::json!({"remove": [1]})).expect("diff");
    test.context
        .update_geojson_data("shapes", &removal)
        .expect("remove by id");
    test.frame(1);
    test.receive().await;
    assert!(!test.loaded(), "the removed feature is gone from the tile");
}

#[tokio::test]
async fn set_data_does_not_cut_short_the_backoff_of_a_failing_tile() {
    let mut test = Fixture::new(Kind::Vector, false).await;
    let url = format!("{}/unstable/shapes.geojson", test.source.url);
    test.context.style = geojson_style(serde_json::Value::String(url.clone()));
    test.frame(0);
    test.receive().await;
    assert_eq!(
        test.source.requests(),
        1,
        "the first attempt failed with a 503"
    );
    test.context
        .set_geojson_data("shapes", crate::style::source::GeoJsonData::Url(url))
        .expect("set data");
    test.frame(1);
    assert_eq!(
        test.kernel.apc().pending(),
        0,
        "the tile keeps waiting for its own retry deadline"
    );
    test.frame(1000);
    test.frame(1001);
    assert_eq!(test.kernel.apc().pending(), 1);
}

fn state_style() -> crate::style::Style {
    let mut style: crate::style::Style = serde_json::from_value(serde_json::json!({"version":8,
        "state":{"show":{"default":true}},
        "sources":{"shapes":{"type":"geojson","data":serde_json::from_str::<serde_json::Value>(WORLD_POLYGON).expect("polygon")}},
        "layers":[
            {"id":"toggled","source":"shapes","type":"fill","paint":{"fill-color":"#00ff00"},
             "filter":["==",["global-state","show"],true]},
            {"id":"steady","source":"shapes","type":"fill","paint":{"fill-color":"#0000ff"}}]}))
    .expect("style with state");
    style.resolve_global_state();
    style
}

fn layers_with_geometry(test: &Fixture) -> Vec<String> {
    let Some(component) = test
        .context
        .world
        .tiles
        .query::<&crate::vector::VectorLayerBucketComponent>(Default::default())
    else {
        return Vec::new();
    };
    let mut ids: Vec<String> = component
        .layers
        .iter()
        .filter_map(|layer| match layer {
            crate::vector::VectorLayerBucket::AvailableLayer(bucket)
                if !bucket.buffer.buffer.indices.is_empty() =>
            {
                Some(bucket.style_layer_id.clone())
            }
            _ => None,
        })
        .collect();
    ids.sort();
    ids
}

#[tokio::test]
async fn global_state_changes_only_the_layers_that_read_it_and_only_when_they_do() {
    let mut test = Fixture::new(Kind::Vector, false).await;
    test.context.style = state_style();
    test.frame(0);
    test.receive().await;
    assert_eq!(layers_with_geometry(&test), ["steady", "toggled"]);

    test.context
        .set_global_state("unrelated", serde_json::json!(1));
    test.frame(1);
    assert_eq!(
        test.kernel.apc().pending(),
        0,
        "a key no layer reads does not touch the tiles"
    );

    test.context
        .set_global_state("show", serde_json::json!(false));
    test.frame(2);
    assert_eq!(
        test.kernel.apc().pending(),
        1,
        "a dependent layer refreshes its tile"
    );
    test.receive().await;
    assert_eq!(
        layers_with_geometry(&test),
        ["steady"],
        "only the dependent layer changed"
    );

    test.context
        .set_global_state("show", serde_json::Value::Null);
    test.frame(3);
    test.receive().await;
    assert_eq!(
        layers_with_geometry(&test),
        ["steady", "toggled"],
        "null restores the declared default"
    );
}

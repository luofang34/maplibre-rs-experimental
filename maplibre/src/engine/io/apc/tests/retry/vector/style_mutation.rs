use super::{
    super::{
        fixture::{Fixture, Kind},
        source::Response,
    },
    tests::{geojson_style, WORLD_POLYGON},
};

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

#[tokio::test]
async fn a_background_layer_reading_state_does_not_refetch_vector_tiles() {
    let mut test = Fixture::new(Kind::Vector, false).await;
    let mut style = state_style();
    style.state.insert(
        "paper".into(),
        crate::style::state::StateDeclaration {
            default: Some(serde_json::json!("#ffffff")),
        },
    );
    let background: crate::style::layer::StyleLayer = serde_json::from_value(serde_json::json!(
        {"id": "paper", "type": "background",
         "paint": {"background-color": ["global-state", "paper"]}}))
    .expect("background layer");
    style.layers.push(background);
    style.resolve_global_state();
    test.context.style = style;
    test.frame(0);
    test.receive().await;
    test.context
        .set_global_state("paper", serde_json::json!("#000000"));
    test.frame(1);
    assert_eq!(
        test.kernel.apc().pending(),
        0,
        "a background is drawn from the style each frame, not from tiles"
    );
}

fn two_layer_style() -> crate::style::Style {
    serde_json::from_value(serde_json::json!({"version":8,
        "sources":{"shapes":{"type":"geojson","data":serde_json::from_str::<serde_json::Value>(WORLD_POLYGON).expect("polygon")}},
        "layers":[
            {"id":"paper","type":"background","paint":{"background-color":"#ffffff"}},
            {"id":"blue","source":"shapes","type":"fill","paint":{"fill-color":"#0000ff"}}]}))
    .expect("style")
}

fn green_fill(id: &str) -> serde_json::Value {
    serde_json::json!({"id": id, "source": "shapes", "type": "fill",
        "paint": {"fill-color": "#00ff00"}})
}

#[tokio::test]
async fn added_layers_appear_and_removed_layers_disappear_after_the_tiles_refresh() {
    let mut test = Fixture::new(Kind::Vector, false).await;
    test.context.style = two_layer_style();
    test.frame(0);
    test.receive().await;
    assert_eq!(layers_with_geometry(&test), ["blue"]);

    let change = test
        .context
        .mutate_style(|style| style.add_layer(green_fill("green"), None))
        .expect("add layer");
    assert!(change.redraw_tiles);
    test.frame(1);
    assert_eq!(
        test.kernel.apc().pending(),
        1,
        "the loaded tile is requested again"
    );
    test.receive().await;
    assert_eq!(layers_with_geometry(&test), ["blue", "green"]);

    test.context
        .mutate_style(|style| style.remove_layer("blue"))
        .expect("remove layer");
    test.frame(2);
    test.receive().await;
    assert_eq!(
        layers_with_geometry(&test),
        ["green"],
        "the removed layer's geometry is gone"
    );

    test.context
        .mutate_style(|style| {
            style.add_layer(
                serde_json::json!({"id": "blue", "source": "shapes",
            "type": "fill", "paint": {"fill-color": "#0000ff"}}),
                Some("green"),
            )
        })
        .expect("the same id can come back");
    test.frame(3);
    test.receive().await;
    assert_eq!(layers_with_geometry(&test), ["blue", "green"]);
}

#[tokio::test]
async fn a_paint_change_shows_in_the_pixels_and_a_refused_change_costs_nothing() {
    let mut test = Fixture::new(Kind::Vector, false).await;
    test.context.style = two_layer_style();
    let mut frames = super::render::Frames::new(&mut test);
    test.frame(0);
    test.receive().await;
    test.context
        .mutate_style(|style| {
            style.set_paint_property("blue", "fill-color", serde_json::json!("#00ff00"))
        })
        .expect("set paint");
    test.frame(1);
    test.receive().await;
    super::render::assert_green(&frames.render(&mut test));

    let refused = test.context.mutate_style(|style| {
        style.set_paint_property("blue", "fill-color", serde_json::json!(["not-an-operator"]))
    });
    assert!(refused.is_err());
    let missing = test.context.mutate_style(|style| {
        style.add_layer(
            serde_json::json!({"id": "x", "type": "fill", "source": "nowhere"}),
            None,
        )
    });
    assert!(missing.is_err());
    test.frame(2);
    assert_eq!(
        test.kernel.apc().pending(),
        0,
        "refused changes touch neither style nor tiles"
    );
    test.frame(3);
    super::render::assert_green(&frames.render(&mut test));
}

#[tokio::test]
async fn reordering_and_background_edits_refresh_only_what_needs_it() {
    let mut test = Fixture::new(Kind::Vector, false).await;
    test.context.style = two_layer_style();
    test.context
        .mutate_style(|style| style.add_layer(green_fill("green"), None))
        .expect("add");
    test.frame(0);
    test.receive().await;

    test.context
        .mutate_style(|style| {
            style.set_paint_property("paper", "background-color", serde_json::json!("#101010"))
        })
        .expect("background");
    test.frame(1);
    assert_eq!(
        test.kernel.apc().pending(),
        0,
        "a background is redrawn from the style"
    );

    test.context
        .mutate_style(|style| style.move_layer("green", Some("blue")))
        .expect("reorder");
    let order: Vec<_> = test
        .context
        .style
        .layers
        .iter()
        .map(|layer| (layer.id.clone(), layer.index))
        .collect();
    assert_eq!(
        order,
        [("paper".into(), 0), ("green".into(), 1), ("blue".into(), 2)]
    );
    test.frame(2);
    assert_eq!(
        test.kernel.apc().pending(),
        0,
        "draw order updates GPU layer z-index metadata in-place without refetching tiles"
    );
    assert_eq!(layers_with_geometry(&test), ["blue", "green"]);
}

#[tokio::test]
async fn a_geojson_source_removed_and_added_again_shows_only_its_new_data() {
    let mut test = Fixture::new(Kind::Vector, false).await;
    test.context.style = two_layer_style();
    test.frame(0);
    test.receive().await;
    assert_eq!(layers_with_geometry(&test), ["blue"]);

    assert!(
        test.context
            .mutate_style(|style| style.remove_source("shapes"))
            .is_err(),
        "a source in use cannot be removed"
    );
    test.context
        .mutate_style(|style| style.remove_layer("blue"))
        .expect("remove layer");
    test.context
        .mutate_style(|style| style.remove_source("shapes"))
        .expect("remove source");
    test.frame(1);
    test.receive().await;
    assert!(layers_with_geometry(&test).is_empty());

    let nothing: crate::style::source::Source = serde_json::from_value(serde_json::json!(
        {"type": "geojson", "data": {"type": "FeatureCollection", "features": []}}))
    .expect("source");
    test.context
        .mutate_style(|style| style.add_source("shapes", nothing))
        .expect("add source");
    test.context
        .mutate_style(|style| style.add_layer(green_fill("green"), None))
        .expect("add layer");
    test.frame(2);
    test.receive().await;
    assert!(
        layers_with_geometry(&test).is_empty(),
        "the readded source has its own empty data, not the removed source's polygon"
    );
}

#[tokio::test]
async fn a_layer_removed_while_its_tile_is_in_flight_does_not_come_back() {
    let mut test = Fixture::new(Kind::Vector, false).await;
    test.context.style = two_layer_style();
    test.frame(0);
    assert_eq!(test.kernel.apc().pending(), 1, "the first tile is loading");
    test.context
        .mutate_style(|style| style.remove_layer("blue"))
        .expect("remove while loading");
    test.receive().await;
    assert!(
        layers_with_geometry(&test).is_empty(),
        "the reply that was already on its way is dropped for the removed layer"
    );
    test.frame(1);
    test.receive().await;
    test.frame(2);
    test.receive().await;
    assert!(
        layers_with_geometry(&test).is_empty(),
        "and the refetch does not restore it"
    );
}

#[tokio::test]
async fn a_removed_url_source_is_not_requested_again_and_a_readded_one_loads_its_own_data() {
    let mut test = Fixture::new(Kind::Vector, false).await;
    let url = format!("{}/unstable/shapes.geojson", test.source.url);
    test.context.style = geojson_style(serde_json::Value::String(url.clone()));
    test.frame(0);
    test.receive().await;
    assert_eq!(
        test.source.requests(),
        1,
        "the first attempt failed with a 503 and backs off"
    );

    test.context
        .mutate_style(|style| style.remove_layer("area"))
        .expect("remove layer");
    test.context
        .mutate_style(|style| style.remove_source("shapes"))
        .expect("remove source");
    test.frame(1000);
    test.frame(1001);
    test.receive().await;
    assert_eq!(
        test.source.requests(),
        1,
        "nothing asks the removed source's URL again at the retry deadline"
    );

    test.source
        .set(Response::Bytes(WORLD_POLYGON.as_bytes().to_vec()));
    let readded: crate::style::source::Source =
        serde_json::from_value(serde_json::json!({"type": "geojson", "data": url}))
            .expect("source");
    test.context
        .mutate_style(|style| style.add_source("shapes", readded))
        .expect("add source");
    test.context
        .mutate_style(|style| {
            style.add_layer(
                serde_json::json!({"id": "area", "source": "shapes", "type": "fill",
                    "paint": {"fill-color": "#00ff00"}}),
                None,
            )
        })
        .expect("add layer");
    test.frame(2000);
    test.receive().await;
    assert_eq!(
        layers_with_geometry(&test),
        ["area"],
        "the readded source loads its own data"
    );
    assert_eq!(test.source.requests(), 2);
}

#[tokio::test]
async fn the_worker_index_reaches_the_world_for_a_style_declared_source() {
    let mut test = Fixture::new(Kind::Vector, false).await;
    test.context.style = geojson_style(serde_json::from_str(WORLD_POLYGON).expect("polygon"));
    test.frame(0);
    test.receive().await;
    assert!(
        test.context.world.tiles.geometry_index.approximate_bytes() > 0,
        "the polygon is indexed for queries"
    );
}

#[tokio::test]
async fn every_source_of_a_tile_keeps_its_own_index() {
    let mut test = Fixture::new(Kind::Vector, true).await;
    test.source.set(Response::Bytes(super::tile()));
    test.source.set_healthy(Response::Bytes(super::tile()));
    test.frame(0);
    test.receive().await;
    test.receive().await;
    let sources: Vec<Option<String>> = test
        .context
        .world
        .tiles
        .geometry_index
        .tile_indexes(&Default::default())
        .expect("indexed tile")
        .map(|(source, _)| source.map(str::to_owned))
        .collect();
    assert_eq!(
        sources,
        [Some("healthy".to_owned()), Some("source".to_owned())],
        "the second source's index did not replace the first's"
    );
}

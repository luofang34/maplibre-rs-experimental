//! A change to a GeoJSON source's data refreshes its layers without fetching the tile's other
//! sources again.
use super::{
    super::{
        fixture::{Fixture, Kind},
        source::Response,
    },
    render::{assert_center, Frames},
    tests::{inline, NOTHING, WORLD_POLYGON},
};
use crate::vector::{VectorLayerBucket, VectorLayerBucketComponent};

/// A base map fetched from `path` of the fake server under an inline GeoJSON polygon.
fn style(url: &str, path: &str) -> crate::style::Style {
    let polygon: serde_json::Value = serde_json::from_str(WORLD_POLYGON).expect("polygon");
    serde_json::from_value(serde_json::json!({"version":8,
        "sources":{
            "base":{"type":"vector","tiles":[format!("{url}/{path}/{{z}}/{{x}}/{{y}}")],"maxzoom":0},
            "shapes":{"type":"geojson","data":polygon}},
        "layers":[
            {"id":"base","source":"base","source-layer":"land","type":"fill","paint":{"fill-color":"#ff0000"}},
            {"id":"area","source":"shapes","type":"fill","paint":{"fill-color":"#00ff00"}}]}))
    .expect("style")
}

fn drawn_layers(test: &Fixture) -> Vec<String> {
    let mut drawn: Vec<String> = test
        .context
        .world
        .tiles
        .query::<&VectorLayerBucketComponent>(Default::default())
        .expect("tile")
        .layers
        .iter()
        .filter_map(|layer| match layer {
            VectorLayerBucket::AvailableLayer(bucket)
                if !bucket.buffer.buffer.indices.is_empty() =>
            {
                Some(bucket.style_layer_id.clone())
            }
            _ => None,
        })
        .collect();
    drawn.sort_unstable();
    drawn
}

#[tokio::test]
async fn new_geojson_data_refreshes_its_layers_without_fetching_the_other_sources_again() {
    let mut test = Fixture::new(Kind::Vector, false).await;
    test.context.style = style(&test.source.url, "healthy");
    test.source.set_healthy(Response::Bytes(super::tile()));
    let mut frames = Frames::new(&mut test);
    test.frame(0);
    test.receive().await;
    assert_eq!(test.source.requests_for("/healthy/"), 1);
    assert_eq!(drawn_layers(&test), ["area", "base"]);
    assert_center(
        &frames.render(&mut test),
        [0, 255, 0, 255],
        "the GeoJSON polygon covers the base map",
    );

    test.context
        .set_geojson_data("shapes", inline(NOTHING))
        .expect("set data");
    test.frame(1000);
    assert_eq!(
        test.kernel.apc().pending(),
        1,
        "the tile is requested again"
    );
    test.receive().await;
    assert_eq!(
        test.source.requests_for("/healthy/"),
        1,
        "the base map is not fetched again for a change to the GeoJSON source"
    );
    // Rendering uploads the replacement, which then takes the old layer's place.
    assert_center(
        &frames.render(&mut test),
        [255, 0, 0, 255],
        "the base map shows through where the polygon was",
    );
    assert_eq!(
        drawn_layers(&test),
        ["base"],
        "the base map keeps its geometry and the GeoJSON layer is cleared"
    );

    test.context
        .set_geojson_data("shapes", inline(WORLD_POLYGON))
        .expect("set data again");
    test.frame(2000);
    assert_eq!(
        test.kernel.apc().pending(),
        1,
        "the tile is requested again"
    );
    test.receive().await;
    assert_eq!(test.source.requests_for("/healthy/"), 1);
    assert_center(
        &frames.render(&mut test),
        [0, 255, 0, 255],
        "the new polygon is drawn over the kept base map",
    );
    assert_eq!(drawn_layers(&test), ["area", "base"]);
}

#[tokio::test]
async fn a_retry_of_a_source_that_failed_is_not_narrowed_to_a_geojson_change_made_meanwhile() {
    let mut test = Fixture::new(Kind::Vector, false).await;
    test.context.style = style(&test.source.url, "unstable");
    test.source.set(Response::Bytes(super::tile()));
    test.frame(0);
    test.receive().await;
    assert_eq!(test.source.requests_for("/unstable/"), 1);
    // A refresh of the whole tile while the base map answers 503: the tile keeps what it
    // shows and backs off.
    test.source.set(Response::Status(503));
    crate::io::tile_retry::refresh(
        &mut test.context.world,
        crate::io::tile_retry::RequestKind::Vector,
    );
    test.frame(1);
    test.receive().await;
    assert_eq!(test.source.requests_for("/unstable/"), 2);
    // New GeoJSON data inside the back-off window must not turn the retry into a request
    // for the GeoJSON source alone, or the base map would never be asked for again.
    test.context
        .set_geojson_data("shapes", inline(NOTHING))
        .expect("set data");
    test.source.set(Response::Bytes(super::tile()));
    test.frame(1001);
    test.receive().await;
    assert_eq!(
        test.source.requests_for("/unstable/"),
        3,
        "the retry fetches the base map again, as the attempt that failed did"
    );
}

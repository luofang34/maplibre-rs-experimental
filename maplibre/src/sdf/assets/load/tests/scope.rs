//! A source's tile asks for the assets of that source's own layers, as its tile is cut.

use super::*;
use crate::{
    coords::{WorldTileCoords, ZoomLevel},
    geojson::index::GeoJsonIndex,
    io::tile_sources::{source_layer_groups, SourceLayerGroup, TileKind},
    style::source::Source,
};

fn group<'s>(groups: &'s [SourceLayerGroup], name: &str) -> &'s SourceLayerGroup {
    groups
        .iter()
        .find(|group| group.source_name.as_deref() == Some(name))
        .expect("the source's group")
}

fn requested(server: &Server) -> Vec<String> {
    server.urls.lock().expect("urls").clone()
}

#[tokio::test]
async fn a_geojson_label_without_a_source_layer_asks_for_its_font_and_icon() {
    let style: Style = serde_json::from_value(serde_json::json!({"version":8,
        "glyphs":"https://fonts.invalid/{fontstack}/{range}.pbf","sprite":"https://sprites.invalid/s",
        "sources":{"points":{"type":"geojson","data":{"type":"Feature",
            "properties":{"name":"Ж A"},"geometry":{"type":"Point","coordinates":[10.0,10.0]}}}},
        "layers":[{"id":"label","type":"symbol","source":"points",
            "layout":{"text-field":["get","name"],"text-font":["Font G"],"icon-image":"marker"}}]}))
    .expect("style");
    let groups = source_layer_groups(&style, TileKind::Vector);
    let points = group(&groups, "points");
    let Some(Source::GeoJson(source)) = style.sources.get("points") else {
        panic!("a GeoJSON source");
    };
    let crate::style::source::GeoJsonData::Inline(data) = &source.data else {
        panic!("inline data");
    };
    let index = GeoJsonIndex::from_value(data, source).expect("index");
    let tile = index.tile(WorldTileCoords {
        x: 0,
        y: 0,
        z: ZoomLevel::from(0),
    });
    let (server, client) = server(None);
    let atlas = load_symbol_assets(
        &client,
        SymbolAssetConfig::of(&style),
        &points.layers,
        &tile,
        0.,
    )
    .await
    .expect("atlas");
    let urls = requested(&server);
    for wanted in [
        "https://fonts.invalid/Font%20G/0-255.pbf",
        "https://fonts.invalid/Font%20G/1024-1279.pbf",
        "https://sprites.invalid/s.json",
    ] {
        assert!(urls.iter().any(|url| url == wanted), "{wanted} in {urls:?}");
    }
    assert!(atlas.glyphs["Font G"].contains_key(&u32::from('A')));
    assert!(atlas.icons.contains_key("marker"));
}

#[tokio::test]
async fn a_source_layer_of_the_same_name_in_another_source_asks_for_nothing() {
    let style: Style = serde_json::from_value(serde_json::json!({"version":8,
        "glyphs":"https://fonts.invalid/{fontstack}/{range}.pbf","sprite":"https://sprites.invalid/s",
        "sources":{
            "a":{"type":"vector","tiles":["https://a.invalid/{z}/{x}/{y}.pbf"]},
            "b":{"type":"vector","tiles":["https://b.invalid/{z}/{x}/{y}.pbf"]}},
        "layers":[
            {"id":"a-label","type":"symbol","source":"a","source-layer":"places",
                "layout":{"text-field":"A","text-font":["Font A"]}},
            {"id":"b-label","type":"symbol","source":"b","source-layer":"places",
                "layout":{"text-field":"Ж","text-font":["Font B"],"icon-image":"marker"}}]}))
    .expect("style");
    let (_, tile) = labelled_tile_style("marker");
    let groups = source_layer_groups(&style, TileKind::Vector);
    let (server, client) = server(None);
    let atlas = load_symbol_assets(
        &client,
        SymbolAssetConfig::of(&style),
        &group(&groups, "a").layers,
        &tile,
        12.,
    )
    .await
    .expect("atlas");
    assert_eq!(
        requested(&server),
        ["https://fonts.invalid/Font%20A/0-255.pbf"],
        "source a's tile asks only for source a's font"
    );
    assert!(!atlas.glyphs.contains_key("Font B"));
    assert!(atlas.icons.is_empty());
}

use std::sync::Arc;

use geozero::mvt::{tile, Message as _, Tile};

use super::{
    process_vector_tile, zigzag, CollectingContext, DefaultLayerIndexed, DefaultLayerTessellated,
    DefaultVectorTransferables, LayerIndexed, LayerTessellated, ProcessVectorContext,
    ProjectionType, StyleLayer, TileIndex, VectorTileRequest, WorldTileCoords,
};

#[test]
fn multiple_style_layers_and_query_index_share_one_feature_properties_allocation() {
    let far = zigzag(2048);
    let bytes = Tile {
        layers: vec![tile::Layer {
            version: 2,
            name: "landuse".to_string(),
            features: vec![tile::Feature {
                id: Some(42),
                tags: vec![0, 0, 1, 1],
                r#type: Some(tile::GeomType::Polygon as i32),
                geometry: vec![9, 0, 0, 26, far, 0, 0, far, zigzag(-2048), 0, 15],
            }],
            keys: vec!["class".to_string(), "name".to_string()],
            values: vec![
                tile::Value {
                    string_value: Some("park".to_string()),
                    ..Default::default()
                },
                tile::Value {
                    string_value: Some("Central Park".to_string()),
                    ..Default::default()
                },
            ],
            extent: Some(4096),
        }],
    }
    .encode_to_vec();

    let fill: StyleLayer = serde_json::from_value(serde_json::json!({
        "id": "park-fill",
        "type": "fill",
        "source": "osm",
        "source-layer": "landuse",
        "paint": {"fill-color": ["match", ["get", "class"], "park", "#00ff00", "#888888"]}
    }))
    .expect("fill layer");
    let outline: StyleLayer = serde_json::from_value(serde_json::json!({
        "id": "park-outline",
        "type": "line",
        "source": "osm",
        "source-layer": "landuse",
        "paint": {"line-color": ["match", ["get", "class"], "park", "#004400", "#444444"], "line-width": 2}
    }))
    .expect("line layer");
    let constant_fill: StyleLayer = serde_json::from_value(serde_json::json!({
        "id": "park-constant",
        "type": "fill",
        "source": "osm",
        "source-layer": "landuse",
        "paint": {"fill-color": "#00ff00"}
    }))
    .expect("constant fill layer");

    let mut processor = ProcessVectorContext::<DefaultVectorTransferables, CollectingContext>::new(
        CollectingContext::default(),
    );
    process_vector_tile(
        &bytes,
        VectorTileRequest {
            coords: WorldTileCoords::default(),
            layers: [fill, outline, constant_fill].into_iter().collect(),
            projection: ProjectionType::Mercator,
            overscaled_zoom: 0,
        },
        &mut processor,
    )
    .expect("tile processes");

    let mut paint_arcs = Vec::new();
    let mut saw_constant = false;
    let mut index_arc = None;
    for message in processor.take_context().0.take() {
        if message.has_tag(DefaultLayerTessellated::message_tag()) {
            let mut layer = *message
                .into_transferable::<DefaultLayerTessellated>()
                .expect("tessellated layer");
            if layer.style_layer_id() == "park-constant" {
                assert!(
                    layer.take_feature_paint().is_none(),
                    "constant-paint layers must not retain per-feature property entries"
                );
                saw_constant = true;
            } else {
                let entries = layer.take_feature_paint().expect("feature paint entries");
                assert!(!entries.is_empty());
                paint_arcs.push(entries[0].properties.clone());
            }
        } else if message.has_tag(DefaultLayerIndexed::message_tag()) {
            let indexed = *message
                .into_transferable::<DefaultLayerIndexed>()
                .expect("indexed layer");
            let TileIndex::Linear { list } = indexed.to_tile_index() else {
                panic!("expected linear tile index");
            };
            assert_eq!(list.len(), 1);
            index_arc = Some(list[0].properties.clone());
        }
    }

    assert!(saw_constant, "constant layer was tessellated");
    assert_eq!(
        paint_arcs.len(),
        2,
        "both data-driven style layers produced paint entries"
    );
    let index_arc = index_arc.expect("indexed geometry present");
    assert!(
        Arc::ptr_eq(&paint_arcs[0], &paint_arcs[1]),
        "multiple style layers on the same source-layer must share one FeatureProperties allocation"
    );
    assert!(
        Arc::ptr_eq(&paint_arcs[0], &index_arc),
        "query index and per-layer feature paint must share the same FeatureProperties allocation"
    );
}

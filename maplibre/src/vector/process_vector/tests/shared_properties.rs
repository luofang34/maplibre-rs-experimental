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
                let entries = layer.take_feature_paint().unwrap_or_default();
                assert!(
                    entries
                        .iter()
                        .all(|entry| entry.raw_id.is_none() && entry.properties.is_empty()),
                    "constant-paint layers keep slot kinds at most, never a feature's id or properties"
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

#[test]
fn decode_tile_for_layers_skips_unrequested_mvt_layers_and_tolerates_padding() {
    let mut bytes = Tile {
        layers: vec![
            tile::Layer {
                version: 2,
                name: "housenumber".to_string(),
                features: vec![tile::Feature {
                    id: Some(1),
                    r#type: Some(tile::GeomType::Point as i32),
                    geometry: vec![9, 10, 10],
                    ..Default::default()
                }],
                extent: Some(4096),
                ..Default::default()
            },
            tile::Layer {
                version: 2,
                name: "building".to_string(),
                features: vec![tile::Feature {
                    id: Some(2),
                    r#type: Some(tile::GeomType::Point as i32),
                    geometry: vec![9, 20, 20],
                    ..Default::default()
                }],
                extent: Some(4096),
                ..Default::default()
            },
        ],
    }
    .encode_to_vec();
    bytes.extend([0_u8; 32]);

    let filtered =
        crate::vector::process_vector::decode_tile_for_layers(&bytes, |name| name == "building")
            .expect("filtered decode succeeds");
    assert_eq!(filtered.layers.len(), 1);
    assert_eq!(filtered.layers[0].name, "building");
    assert_eq!(filtered.layers[0].features.len(), 1);
    assert_eq!(filtered.layers[0].features[0].id, Some(2));

    let empty =
        crate::vector::process_vector::decode_tile_for_layers(&bytes, |name| name == "water")
            .expect("empty layer filter succeeds");
    assert!(empty.layers.is_empty());

    assert!(
        crate::vector::process_vector::decode_tile_for_layers(&[0x0a, 0xff], |_| true).is_err()
    );
}

#[test]
fn sibling_uniform_line_layers_share_one_tessellated_vertex_buffer() {
    let bytes = super::diagonal_line_tile(4096);
    let mut casing: StyleLayer = serde_json::from_value(serde_json::json!({
        "id": "airways-casing",
        "type": "line",
        "source": "chart",
        "source-layer": "airways",
        "paint": {"line-color": "#112233", "line-width": 4}
    }))
    .expect("casing layer");
    casing.index = 0;
    let mut core: StyleLayer = serde_json::from_value(serde_json::json!({
        "id": "airways-core",
        "type": "line",
        "source": "chart",
        "source-layer": "airways",
        "filter": ["==", ["get", "level"], "low"],
        "paint": {"line-color": "#ffaa00", "line-width": 2}
    }))
    .expect("core layer");
    core.index = 1;

    let mut processor = ProcessVectorContext::<DefaultVectorTransferables, CollectingContext>::new(
        CollectingContext::default(),
    );
    process_vector_tile(
        &bytes,
        VectorTileRequest {
            coords: WorldTileCoords::from((1, 1, 2_u8.into())),
            layers: [casing, core].into_iter().collect(),
            projection: ProjectionType::Mercator,
            overscaled_zoom: 0,
        },
        &mut processor,
    )
    .expect("tile processes");

    let layers = super::tessellated(processor.take_context().0.take());
    assert_eq!(layers.len(), 2);
    let primary = layers
        .iter()
        .find(|l| l.style_layer_id() == "airways-casing")
        .expect("primary layer");
    let secondary = layers
        .iter()
        .find(|l| l.style_layer_id() == "airways-core")
        .expect("secondary layer");

    let key = primary
        .buffer
        .shared_geometry_key()
        .expect("shared geometry key assigned");
    assert_eq!(secondary.buffer.shared_geometry_key(), Some(key));
    assert!(
        !primary.buffer.buffer.vertices.is_empty(),
        "primary sibling carries the shared tessellated vertices"
    );
    assert!(
        secondary.buffer.buffer.vertices.is_empty(),
        "secondary sibling omits duplicate vertices across worker wire"
    );
    assert_eq!(
        primary.buffer.buffer.indices,
        secondary.buffer.buffer.indices
    );
    assert!(secondary.buffer.usable_indices > 0);
}

//! One feature whose geometry cannot be read does not take its layer with it.
use super::*;

#[test]
fn a_feature_whose_geometry_cannot_be_read_is_left_out_without_its_layer() {
    let polygon = |geometry: Vec<u32>| tile::Feature {
        id: None,
        tags: vec![],
        r#type: Some(tile::GeomType::Polygon as i32),
        geometry,
    };
    let bytes = Tile {
        layers: vec![tile::Layer {
            version: 2,
            name: "land".into(),
            features: vec![
                // A square, and a ring of three points on one line, which no reader can take
                // for an exterior ring.
                polygon(vec![9, 0, 0, 26, 8192, 0, 0, 8192, 8191, 0, 15]),
                polygon(vec![9, 0, 0, 18, 200, 0, 200, 0, 15]),
            ],
            keys: vec![],
            values: vec![],
            extent: Some(4096),
        }],
    }
    .encode_to_vec();
    let fill: StyleLayer = serde_json::from_value(serde_json::json!({
        "id": "land", "type": "fill", "source": "chart", "source-layer": "land",
        "paint": {"fill-color": "#00ff00"}
    }))
    .expect("valid style layer");
    let messages = process(&bytes, fill);
    assert!(
        !messages
            .iter()
            .any(|message| message.has_tag(DefaultLayerMissing::message_tag())),
        "the layer is not reported missing"
    );
    let layers = tessellated(messages);
    assert_eq!(layers.len(), 1);
    assert!(
        !layers[0].buffer.buffer.indices.is_empty(),
        "the square is tessellated"
    );
    assert_eq!(
        layers[0].layer_data.features.len(),
        1,
        "the unreadable feature is left out of the layer data too"
    );
}

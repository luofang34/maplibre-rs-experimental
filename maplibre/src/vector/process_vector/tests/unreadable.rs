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
        layers[0].feature_indices.len(),
        2,
        "only the readable feature produces tessellated fill + default-outline metadata (2 entries, not 4)"
    );
    let entries = layers[0].feature_paint.as_deref().unwrap_or_default();
    assert_eq!(
        entries
            .iter()
            .map(|entry| (entry.kind, entry.raw_id, entry.properties.is_empty()))
            .collect::<Vec<_>>(),
        [
            (
                crate::vector::feature_paint::BucketEntryKind::Feature,
                None,
                true
            ),
            (
                crate::vector::feature_paint::BucketEntryKind::FillOutline,
                None,
                true
            )
        ],
        "a constant fill keeps which slot is the outline, and no feature's id or properties"
    );
    assert!(layers[0].layer_data.features.is_empty());
}

#[test]
fn a_building_base_whose_parts_all_start_above_ground_is_not_hidden_by_hide_3d() {
    let square_geom = |x0: i32, y0: i32, size: i32| {
        let zz = |v: i32| ((v << 1) ^ (v >> 31)) as u32;
        vec![
            (1 << 3) | 1,
            zz(x0),
            zz(y0),
            (3 << 3) | 2,
            zz(size),
            0,
            0,
            zz(size),
            zz(-size),
            0,
            (1 << 3) | 7,
        ]
    };
    let tile_bytes = Tile {
        layers: vec![tile::Layer {
            version: 2,
            name: "building".into(),
            extent: Some(4096),
            keys: vec![
                "hide_3d".into(),
                "render_min_height".into(),
                "render_height".into(),
            ],
            values: vec![
                tile::Value {
                    bool_value: Some(true),
                    ..Default::default()
                },
                tile::Value {
                    double_value: Some(0.0),
                    ..Default::default()
                },
                tile::Value {
                    double_value: Some(120.0),
                    ..Default::default()
                },
                tile::Value {
                    double_value: Some(238.0),
                    ..Default::default()
                },
            ],
            features: vec![
                tile::Feature {
                    id: Some(1),
                    r#type: Some(3),
                    tags: vec![0, 0, 1, 1, 2, 2],
                    geometry: square_geom(100, 100, 200),
                },
                tile::Feature {
                    id: Some(2),
                    r#type: Some(3),
                    tags: vec![1, 2, 2, 3],
                    geometry: square_geom(150, 150, 80),
                },
                tile::Feature {
                    id: Some(3),
                    r#type: Some(3),
                    tags: vec![0, 0, 1, 1, 2, 2],
                    geometry: square_geom(1000, 1000, 200),
                },
                tile::Feature {
                    id: Some(4),
                    r#type: Some(3),
                    tags: vec![1, 1, 2, 2],
                    geometry: square_geom(1050, 1050, 80),
                },
            ],
        }],
    }
    .encode_to_vec();

    let layer: StyleLayer = serde_json::from_value(serde_json::json!({
        "id": "3d-buildings",
        "type": "fill-extrusion",
        "source": "openmaptiles",
        "source-layer": "building",
        "filter": ["!=", ["get", "hide_3d"], true],
        "paint": {
            "fill-extrusion-height": ["get", "render_height"],
            "fill-extrusion-base": ["get", "render_min_height"]
        }
    }))
    .expect("valid extrusion layer");
    let layers = tessellated(process(&tile_bytes, layer));
    assert_eq!(layers[0].feature_indices.len(), 3);
}

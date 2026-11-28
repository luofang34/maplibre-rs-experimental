use std::sync::Arc;

use maplibre::{
    coords::{WorldTileCoords, ZoomLevel},
    io::{
        apc::IntoMessage,
        geometry_index::{FeatureMeta, IndexedGeometry, TileIndex},
    },
    render::ShaderVertex,
    sdf::SymbolLayersDataComponent,
    style::{
        expression::{FeatureProperties, Value, FEATURE_STATE_PREFIX},
        feature_state::FeatureStateTarget,
        Style,
    },
    tcs::world::World,
    tile::Layer,
    vector::{
        apply_vector_worker_batch,
        feature_paint::{BucketEntryKind, BucketFeatureEntry, TileFeaturePaintComponent},
        tessellation::OverAlignedVertexBuffer,
        LayerIndexed, LayerTessellated, TessellatedLayerPayload, VectorLayerBucketComponent,
    },
};
use serde_json::json;
use wasm_bindgen_test::*;

use super::{super::FlatTransferables, FlatBufferTransferable};

#[wasm_bindgen_test]
fn layer_tessellated_preserves_all_shader_vertex_fields_and_feature_paint() {
    let packed_elevation = f32::from_bits(0xFF00_1234);
    let input = ShaderVertex {
        position: [128.5, 2048.25],
        normal: [-0.5, 0.75],
        distance: 42.0,
        elevation: packed_elevation,
        edge_distance: 19.5,
        corner_code: 17.0,
    };
    let buffer = OverAlignedVertexBuffer::from_iters([input, input, input], [0_u32, 1, 2], 3);
    let layer = Layer {
        version: 2,
        name: "boundary".into(),
        features: Vec::new(),
        keys: Vec::new(),
        values: Vec::new(),
        extent: Some(4096),
    };
    let mut props = FeatureProperties::default();
    props.insert("admin_level".into(), Value::Number(2.0));
    let entry = BucketFeatureEntry {
        raw_id: Some(7),
        properties: Arc::new(props.clone()),
        kind: BucketEntryKind::Feature,
    };
    let mut transferable = <FlatBufferTransferable as LayerTessellated>::build_from_with_paint(
        TessellatedLayerPayload {
            coords: WorldTileCoords {
                x: 2,
                y: 1,
                z: ZoomLevel::new(2),
            },
            buffer,
            feature_indices: vec![3],
            feature_colors: vec![[0.2, 0.4, 0.6, 1.0]],
            feature_sort_keys: vec![],
            layer_data: layer,
            style_layer_id: "countries-boundary".into(),
            entries: vec![entry],
        },
    );
    let paint = transferable
        .take_feature_paint()
        .expect("feature paint survives FlatBuffer roundtrip");
    assert_eq!(paint.len(), 1);
    assert_eq!(paint[0].raw_id, Some(7));
    assert_eq!(*paint[0].properties, props);

    let bucket = transferable.to_bucket();
    let output = bucket.buffer.vertices[0];
    assert_eq!(output.position, input.position);
    assert_eq!(output.normal, input.normal);
    assert_eq!(output.distance, input.distance);
    assert_eq!(output.elevation.to_bits(), input.elevation.to_bits());
    assert_eq!(
        output.edge_distance.to_bits(),
        input.edge_distance.to_bits()
    );
    assert_eq!(output.corner_code, input.corner_code);
}

#[wasm_bindgen_test]
fn batch_of_two_flatbuffer_messages_prevents_aba_and_shares_decoded_maps_across_layers() {
    let coords = WorldTileCoords {
        x: 2,
        y: 1,
        z: ZoomLevel::new(2),
    };
    let mut style: Style = serde_json::from_value(json!({
        "version": 8,
        "sources": {"osm": {"type": "vector"}},
        "layers": [
            {
                "id": "countries-fill",
                "type": "fill",
                "source": "osm",
                "source-layer": "boundary",
                "paint": {"fill-color": ["match", ["get", "name"], "alpha", "#00ff00", "#0000ff"]}
            },
            {
                "id": "countries-line",
                "type": "line",
                "source": "osm",
                "source-layer": "boundary",
                "paint": {"line-color": ["case", ["boolean", ["feature-state", "selected"], false], "#ffffff", "#222222"]}
            }
        ]
    }))
    .expect("valid style");

    let mut selected = serde_json::Map::new();
    selected.insert("selected".into(), json!(true));
    style.set_feature_state(
        FeatureStateTarget::vector("osm", "boundary"),
        7_u64,
        selected,
    );

    let mut world = World::default();
    world
        .tiles
        .spawn_mut(coords)
        .expect("tile")
        .insert(VectorLayerBucketComponent::default())
        .insert(SymbolLayersDataComponent::default());

    let buffer =
        OverAlignedVertexBuffer::from_iters([ShaderVertex::default(); 3], [0_u32, 1, 2], 3);
    let layer = Layer {
        version: 2,
        name: "boundary".into(),
        features: Vec::new(),
        keys: Vec::new(),
        values: Vec::new(),
        extent: Some(4096),
    };
    let mut props_alpha = FeatureProperties::default();
    props_alpha.insert("name".into(), Value::String("alpha".into()));
    let mut props_beta = FeatureProperties::default();
    props_beta.insert("name".into(), Value::String("beta".into()));

    let entries = vec![
        BucketFeatureEntry {
            raw_id: Some(7),
            properties: Arc::new(props_alpha.clone()),
            kind: BucketEntryKind::Feature,
        },
        BucketFeatureEntry {
            raw_id: Some(8),
            properties: Arc::new(props_beta.clone()),
            kind: BucketEntryKind::Feature,
        },
    ];

    let msg_fill = <FlatBufferTransferable as LayerTessellated>::build_from_with_paint(
        TessellatedLayerPayload {
            coords,
            buffer: buffer.clone(),
            feature_indices: vec![3, 3],
            feature_colors: vec![[0.0; 4]; 2],
            feature_sort_keys: vec![],
            layer_data: layer.clone(),
            style_layer_id: "countries-fill".into(),
            entries: entries.clone(),
        },
    );
    let msg_line = <FlatBufferTransferable as LayerTessellated>::build_from_with_paint(
        TessellatedLayerPayload {
            coords,
            buffer,
            feature_indices: vec![3, 3],
            feature_colors: vec![[0.0; 4]; 2],
            feature_sort_keys: vec![],
            layer_data: layer,
            style_layer_id: "countries-line".into(),
            entries,
        },
    );
    let msg_indexed = <FlatBufferTransferable as LayerIndexed>::build_from(
        coords,
        Some("osm".into()),
        TileIndex::Linear {
            list: vec![
                IndexedGeometry::from_point(
                    (64.0, 64.0).into(),
                    FeatureMeta {
                        id: Some(7),
                        source_layer: Arc::from("boundary"),
                        properties: Arc::new(props_alpha),
                        feature_index: 0,
                    },
                )
                .expect("point 7"),
                IndexedGeometry::from_point(
                    (128.0, 128.0).into(),
                    FeatureMeta {
                        id: Some(8),
                        source_layer: Arc::from("boundary"),
                        properties: Arc::new(props_beta),
                        feature_index: 1,
                    },
                )
                .expect("point 8"),
            ],
        },
    );

    apply_vector_worker_batch::<FlatTransferables>(
        &mut world,
        &style,
        vec![
            IntoMessage::into(msg_fill),
            IntoMessage::into(msg_line),
            IntoMessage::into(msg_indexed),
        ],
    )
    .expect("batch applies cleanly");

    let store = world
        .tiles
        .query::<&TileFeaturePaintComponent>(coords)
        .expect("paint store");
    let fill = &store.committed["countries-fill"];
    let line = &store.committed["countries-line"];
    assert_eq!(
        fill[0].properties.get("name"),
        Some(&Value::String("alpha".into()))
    );
    assert_eq!(
        fill[0]
            .properties
            .get(&format!("{FEATURE_STATE_PREFIX}selected")),
        Some(&Value::Bool(true))
    );
    assert_eq!(
        line[1].properties.get("name"),
        Some(&Value::String("beta".into()))
    );
    assert!(
        !line[1]
            .properties
            .contains_key(&format!("{FEATURE_STATE_PREFIX}selected")),
        "second message must not inherit feature 7 properties via reused Arc address"
    );
    assert!(
        Arc::ptr_eq(&fill[0].properties, &line[0].properties)
            && Arc::ptr_eq(&fill[1].properties, &line[1].properties),
        "decoded FlatBuffer messages must share one FeatureProperties map per feature across style layers"
    );
}

#[wasm_bindgen_test]
fn prepacked_wire_round_trips_u16_odd_even_and_u32_dense_fallback() {
    use maplibre::render::shaders::{GpuShaderVertex, PoolVertex};

    let coords = WorldTileCoords {
        x: 7,
        y: 9,
        z: ZoomLevel::new(4),
    };
    let input = ShaderVertex {
        position: [120.25, 340.5],
        normal: [0.5, -0.5],
        distance: 12.0,
        elevation: -1e30,
        edge_distance: 0.0,
        corner_code: 0.0,
    };
    let mut expected_gpu = Vec::new();
    let expected_slice =
        ShaderVertex::encode_vertices("line", &[input, input, input], &mut expected_gpu).to_vec();

    // 1) Prepacked u16 path with odd usable_indices = 3 (pads wire to 4 u16 elements = 8 bytes)
    let odd_buf = OverAlignedVertexBuffer::from_iters([input, input, input], [0_u32, 1, 2], 3);
    let odd_layer = Layer {
        version: 2,
        name: "road".into(),
        ..Default::default()
    };
    let mut odd_msg = <FlatBufferTransferable as LayerTessellated>::build_from_with_paint_typed(
        TessellatedLayerPayload {
            coords,
            buffer: odd_buf,
            feature_indices: vec![3],
            feature_colors: vec![[0.25, 0.5, 0.75, 1.0]],
            feature_sort_keys: Vec::new(),
            layer_data: odd_layer,
            style_layer_id: "road-primary".into(),
            entries: vec![BucketFeatureEntry {
                raw_id: Some(7),
                properties: Default::default(),
                kind: BucketEntryKind::Feature,
            }],
        },
        "line",
        true,
    );
    assert!(odd_msg.take_feature_paint().is_some());
    assert_eq!(odd_msg.style_layer_id(), "road-primary");
    let odd_bucket = odd_msg.to_bucket();
    assert!(odd_bucket.buffer.vertices.is_empty());
    assert!(odd_bucket.buffer.indices.is_empty());
    assert_eq!(odd_bucket.buffer.usable_indices, 3);
    assert_eq!(odd_bucket.buffer.vertex_count(), 3);
    assert_eq!(odd_bucket.buffer.cpu_index_len(), 4);
    let pre_odd = odd_bucket
        .buffer
        .prepacked_gpu()
        .expect("prepacked u16 geometry");
    assert_eq!(pre_odd.vertices, expected_slice);
    assert_eq!(pre_odd.indices_u16.as_deref(), Some(&[0_u16, 1, 2, 0][..]));

    // 2) Prepacked u32 fallback when vertex_count > 65_536
    let dense_verts = vec![input; 65_537];
    let mut fill_gpu = Vec::new();
    let fill_expected =
        <ShaderVertex as PoolVertex>::encode_vertices("fill", &dense_verts[..2], &mut fill_gpu)
            .to_vec();
    let dense_buf = OverAlignedVertexBuffer::from_iters(dense_verts, [0_u32, 65_536, 1, 2], 4);
    let dense_layer = Layer {
        version: 2,
        name: "land".into(),
        ..Default::default()
    };
    let dense_msg = <FlatBufferTransferable as LayerTessellated>::build_from_with_paint_typed(
        TessellatedLayerPayload {
            coords,
            buffer: dense_buf,
            feature_indices: Vec::new(),
            feature_colors: Vec::new(),
            feature_sort_keys: Vec::new(),
            layer_data: dense_layer,
            style_layer_id: "land-fill".into(),
            entries: Vec::new(),
        },
        "fill",
        true,
    );
    let dense_bucket = dense_msg.to_bucket();
    let pre_dense = dense_bucket
        .buffer
        .prepacked_gpu()
        .expect("prepacked dense geometry");
    assert!(pre_dense.indices_u16.is_none());
    assert_eq!(dense_bucket.buffer.vertex_count(), 65_537);
    assert_eq!(&pre_dense.vertices[..2], fill_expected.as_slice());
    assert_eq!(
        pre_dense.indices_u32.as_deref(),
        Some(&[0_u32, 65_536, 1, 2][..])
    );
    assert!(dense_bucket.buffer.indices.is_empty());
    let _: GpuShaderVertex = pre_dense.vertices[0];
}

#![allow(clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use maplibre::{
    benchmarking::tessellation::OverAlignedVertexBuffer,
    coords::WorldTileCoords,
    euclid::{Box2D, Point2D},
    render::shaders::ShaderSymbolVertex,
    sdf::{assets::SymbolAtlas, Feature, SymbolFeatureData},
    style::{expression::empty_feature_properties, filter::GeometryType},
    tile::Layer,
    vector::SymbolLayerTessellated,
};
use wasm_bindgen_test::wasm_bindgen_test;

use super::*;

#[wasm_bindgen_test]
fn parse_symbol_header_uses_total_len_and_roundtrips_bucket() {
    let coords = WorldTileCoords::from((12, 34, 6_u8.into()));
    let vertices = vec![
        ShaderSymbolVertex {
            a_pos_offset: [1, 2, 3, 4],
            a_data: [5, 6, 7, 8],
            a_pixel_and_height: [9, 10],
        };
        4
    ];
    let indices = vec![0_u32, 1, 2, 2, 1, 3];
    let buffer = OverAlignedVertexBuffer::from_vecs(vertices, indices, 6);
    let feature = Feature {
        parts: [None, None, None],
        data: SymbolFeatureData {
            id: Some(99),
            properties: Arc::clone(empty_feature_properties()),
            sort_key: 1.5,
            geometry_type: GeometryType::Point,
            source_feature_index: Some(17),
        },
        bbox: Box2D::new(Point2D::new(-4.0, -4.0), Point2D::new(4.0, 4.0)),
        indices: 0..6,
        text_anchor: Point2D::new(100.0, 200.0),
        str: "Amstel".to_owned(),
        line: None,
        anchor_shifts: vec![],
        text_sets: vec![],
        anchor_sets: vec![],
        text_colors: vec![],
        fallback: false,
        repeat_key: None,
    };
    let atlas = Arc::new(SymbolAtlas::with_wasm_staging([4, 1], vec![255; 16]));
    let layer = Layer {
        version: 2,
        name: "transportation_name".to_owned(),
        extent: Some(4096),
        ..Default::default()
    };
    let transferable = <FlatBufferTransferable as SymbolLayerTessellated>::build_from(
        coords,
        buffer,
        vec![feature],
        Some(atlas),
        layer,
        "road-label".to_owned(),
    );
    let full = transferable.data();
    assert!(full.len() > SYMBOL_HEADER_BYTES);
    let hdr = parse_symbol_header(&full[..SYMBOL_HEADER_BYTES], full.len())
        .expect("40-byte header slice with full byte_len must succeed");
    assert_eq!(hdr.coords, coords);
    assert_eq!(hdr.vert_count, 4);
    assert_eq!(hdr.idx_count, 6);
    assert!(hdr.has_atlas);
    assert!(
        parse_symbol_header(&full[..SYMBOL_HEADER_BYTES], full.len() - 1).is_none(),
        "truncated total_len must be rejected"
    );

    let js_buf = js_sys::Uint8Array::from(full).buffer();
    let direct_bucket = js_wire::decode_symbol_layer_from_js(&js_buf, 0, full.len() as u32)
        .expect("decode_symbol_layer_from_js must decode non-empty symbol layer");
    assert_eq!(direct_bucket.coords, coords);
    assert_eq!(direct_bucket.source_layer, "transportation_name");
    assert_eq!(direct_bucket.style_layer_id, "road-label");
    assert_eq!(direct_bucket.features.len(), 1);
    assert_eq!(direct_bucket.features[0].str, "Amstel");

    let bucket = SymbolLayerTessellated::to_bucket(transferable);
    assert_eq!(bucket.coords, coords);
    assert_eq!(bucket.source_layer, "transportation_name");
    assert_eq!(bucket.style_layer_id, "road-label");
    assert_eq!(bucket.buffer.vertices.len(), 4);
    assert_eq!(bucket.buffer.indices.len(), 6);
    assert_eq!(bucket.features.len(), 1);
    assert_eq!(bucket.features[0].str, "Amstel");
    assert_eq!(bucket.features[0].data.source_feature_index, Some(17));
    assert_eq!(bucket.atlas.as_ref().expect("atlas").size, [4, 1]);
}

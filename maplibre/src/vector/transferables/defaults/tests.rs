use std::sync::Arc;

use geo_types::LineString;
use geozero::mvt::tile::Layer;

use super::*;
use crate::{
    coords::{InnerCoords, ZoomLevel},
    io::geometry_index::{FeatureMeta, IndexedGeometry},
    style::expression::{FeatureProperties, Value},
    vector::feature_paint::BucketEntryKind,
};

fn sample_coords() -> WorldTileCoords {
    WorldTileCoords {
        x: 11,
        y: 22,
        z: ZoomLevel::new(6),
    }
}

fn header_only_layer(name: &str) -> Layer {
    Layer {
        version: 2,
        name: name.to_owned(),
        features: Vec::new(),
        keys: Vec::new(),
        values: Vec::new(),
        extent: Some(4096),
    }
}

#[test]
fn tile_completion_and_missing_layer_messages_roundtrip_with_attempt_metadata() {
    let coords = sample_coords();

    let complete_msg =
        IntoMessage::into(DefaultTileTessellated::build_from(coords)).with_attempt(7);
    assert_eq!(complete_msg.attempt(), Some(7));
    assert_eq!(complete_msg.tag(), DefaultTileTessellated::message_tag());
    let complete = complete_msg
        .into_transferable::<DefaultTileTessellated>()
        .expect("downcasts DefaultTileTessellated");
    assert_eq!(complete.coords(), coords);
    assert!(!complete.pending_symbols());
    assert!(!complete.failed());

    let partial = IntoMessage::into(DefaultTileTessellated::build_partial(coords))
        .into_transferable::<DefaultTileTessellated>()
        .expect("downcasts partial DefaultTileTessellated");
    assert!(partial.pending_symbols());
    assert!(!partial.failed());

    let failed = IntoMessage::into(DefaultTileTessellated::build_failed(coords, true))
        .into_transferable::<DefaultTileTessellated>()
        .expect("downcasts failed DefaultTileTessellated");
    assert!(failed.pending_symbols());
    assert!(failed.failed());

    let missing_msg = IntoMessage::into(DefaultLayerMissing::build_from(coords, "water".into()));
    let missing = missing_msg
        .into_transferable::<DefaultLayerMissing>()
        .expect("downcasts DefaultLayerMissing")
        .to_bucket();
    assert_eq!(missing.coords, coords);
    assert_eq!(missing.source_layer, "water");

    // Wrong target type fails cleanly with MessageError rather than panicking.
    let wrong_msg = IntoMessage::into(DefaultLayerMissing::build_from(coords, "roads".into()));
    let err = wrong_msg
        .into_transferable::<DefaultTileTessellated>()
        .expect_err("mismatched transferable type must return MessageError");
    assert_eq!(err.tag, DefaultLayerMissing::message_tag());
}

#[test]
fn tessellated_symbol_and_indexed_messages_preserve_paint_entries_and_queries() {
    let coords = sample_coords();
    let v0 = ShaderVertex {
        position: [12.0, 34.0],
        normal: [-0.25, 0.75],
        distance: 8.5,
        elevation: 42.0,
        edge_distance: 3.0,
        corner_code: 1.0,
    };
    let buffer = OverAlignedVertexBuffer::from_iters([v0, v0, v0], [0_u32, 1, 2], 3);

    let mut props = FeatureProperties::new();
    props.insert("class".into(), Value::String("residential".into()));
    let paint_entry = BucketFeatureEntry {
        raw_id: Some(108),
        properties: Arc::new(props.clone()),
        kind: BucketEntryKind::Line { length: 64.0 },
    };

    let tessellated = DefaultLayerTessellated::build_from(
        coords,
        buffer,
        vec![3],
        vec![[0.1, 0.2, 0.3, 0.9]],
        vec![12.5],
        header_only_layer("transportation"),
        "roads-casing".into(),
    )
    .with_feature_paint(vec![paint_entry]);

    let mut unpacked = IntoMessage::into(tessellated)
        .with_attempt(99)
        .into_transferable::<DefaultLayerTessellated>()
        .expect("downcasts DefaultLayerTessellated");
    assert!(!unpacked.is_empty());
    assert_eq!(unpacked.coords(), coords);
    assert_eq!(unpacked.style_layer_id(), "roads-casing");

    let extracted_paint = unpacked
        .take_feature_paint()
        .expect("retained feature paint entries survive worker message roundtrip");
    assert_eq!(extracted_paint.len(), 1);
    assert_eq!(extracted_paint[0].raw_id, Some(108));
    assert_eq!(
        extracted_paint[0].kind,
        BucketEntryKind::Line { length: 64.0 }
    );
    assert_eq!(*extracted_paint[0].properties, props);

    let bucket = unpacked.to_bucket();
    assert_eq!(bucket.source_layer, "transportation");
    assert_eq!(bucket.style_layer_id, "roads-casing");
    assert_eq!(bucket.feature_indices, vec![3]);
    assert_eq!(bucket.feature_colors, vec![[0.1, 0.2, 0.3, 0.9]]);
    assert_eq!(bucket.feature_sort_keys, vec![12.5]);
    assert_eq!(bucket.buffer.usable_indices, 3);
    assert_eq!(bucket.buffer.buffer.vertices[0].position, v0.position);

    let empty_symbols = DefaultSymbolLayerTessellated::build_from(
        coords,
        OverAlignedVertexBuffer::empty(),
        Vec::new(),
        None,
        header_only_layer("place"),
        "place-labels".into(),
    );
    let symbol_bucket = IntoMessage::into(empty_symbols)
        .into_transferable::<DefaultSymbolLayerTessellated>()
        .expect("downcasts DefaultSymbolLayerTessellated")
        .to_bucket();
    assert_eq!(symbol_bucket.coords, coords);
    assert_eq!(symbol_bucket.source_layer, "place");
    assert_eq!(symbol_bucket.style_layer_id, "place-labels");
    assert_eq!(symbol_bucket.buffer.usable_indices, 0);

    let indexed_geom = IndexedGeometry::from_linestring(
        LineString::from(vec![(100.0, 256.0), (200.0, 256.0)]),
        FeatureMeta {
            properties: Arc::new(props),
            source_layer: Arc::from("place"),
            id: Some(7),
            feature_index: 0,
        },
    )
    .expect("valid linestring");
    let indexed = DefaultLayerIndexed::build_from(
        coords,
        Some("openmaptiles".into()),
        TileIndex::Linear {
            list: vec![indexed_geom],
        },
    );
    let unpacked_index = IntoMessage::into(indexed)
        .into_transferable::<DefaultLayerIndexed>()
        .expect("downcasts DefaultLayerIndexed");
    assert_eq!(unpacked_index.coords(), coords);
    assert_eq!(unpacked_index.source().as_deref(), Some("openmaptiles"));
    let tile_index = unpacked_index.to_tile_index();
    assert_eq!(
        tile_index
            .point_query(InnerCoords { x: 128.0, y: 256.0 })
            .len(),
        1
    );
}

use maplibre::{
    coords::{InnerCoords, WorldTileCoords, ZoomLevel},
    io::geometry_index::{
        wire::{WireGeometry, WireTileIndex},
        LazyMvtIndex, TileIndex,
    },
    vector::LayerIndexed,
};
use wasm_bindgen_test::*;

use super::FlatBufferTransferable;

#[wasm_bindgen_test]
fn flatbuffer_layer_indexed_roundtrips_spatial_and_source() {
    assert!(<FlatBufferTransferable as LayerIndexed>::requires_geometry_index());
    let wire = WireTileIndex {
        is_spatial: true,
        strings: vec!["road".into()],
        prop_blobs: vec![0, 0, 0, 0],
        prop_offsets: vec![0, 4],
        geometries: vec![WireGeometry {
            id: 99,
            source_layer_idx: 0,
            prop_group_idx: 0,
            feature_index: 3,
            ring_start: 0,
            ring_count: 0,
            coord_start: 0,
            coord_count: 2,
            kind: 1,
            has_id: true,
        }],
        ring_ends: vec![],
        xy: vec![100.0, 200.0, 300.0, 200.0],
    };
    let coords = WorldTileCoords {
        x: 4,
        y: 5,
        z: ZoomLevel::new(3),
    };
    let transferable = <FlatBufferTransferable as LayerIndexed>::build_from(
        coords,
        Some("openmaptiles".into()),
        wire.into_tile_index(),
    );
    assert_eq!(LayerIndexed::coords(&transferable), coords);
    assert_eq!(
        LayerIndexed::source(&transferable).as_deref(),
        Some("openmaptiles")
    );
    let decoded = transferable.to_tile_index();
    let hits = decoded.point_query(InnerCoords { x: 200.0, y: 202.0 });
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, Some(99));
    assert_eq!(&*hits[0].source_layer, "road");
}

#[wasm_bindgen_test]
fn lazy_mvt_layer_indexed_roundtrips_without_eager_decode() {
    use std::sync::Arc;

    let coords = WorldTileCoords {
        x: -2,
        y: 7,
        z: ZoomLevel::new(4),
    };
    let payload = b"packed-mvt-payload";
    let lazy = LazyMvtIndex::from_packed(Arc::from(payload.as_slice()), true);
    let transferable = <FlatBufferTransferable as LayerIndexed>::build_from(
        coords,
        Some("basemap".into()),
        TileIndex::LazyMvt(lazy),
    );
    assert_eq!(LayerIndexed::coords(&transferable), coords);
    assert_eq!(
        LayerIndexed::source(&transferable).as_deref(),
        Some("basemap")
    );
    let decoded = transferable.to_tile_index();
    match decoded {
        TileIndex::LazyMvt(lazy) => {
            let wire = lazy.wire();
            assert!(wire.compressed);
            assert!(lazy.rescale().is_none());
            assert_eq!(wire.bytes, payload);
        }
        _ => panic!("expected LazyMvt variant without eager decode"),
    }
}

#[wasm_bindgen_test]
fn lazy_mvt_layer_indexed_roundtrips_with_rescale() {
    use std::sync::Arc;

    let from = WorldTileCoords {
        x: 1,
        y: 2,
        z: ZoomLevel::new(4),
    };
    let coords = WorldTileCoords {
        x: 5,
        y: 9,
        z: ZoomLevel::new(6),
    };
    let payload = b"overscaled-parent-mvt";
    let lazy = LazyMvtIndex::from_packed(Arc::from(payload.as_slice()), true)
        .with_rescale(Some((from, coords)));
    let transferable = <FlatBufferTransferable as LayerIndexed>::build_from(
        coords,
        Some("openmaptiles".into()),
        TileIndex::LazyMvt(lazy),
    );
    assert_eq!(LayerIndexed::coords(&transferable), coords);
    assert_eq!(
        LayerIndexed::source(&transferable).as_deref(),
        Some("openmaptiles")
    );
    let decoded = transferable.to_tile_index();
    match decoded {
        TileIndex::LazyMvt(lazy) => {
            let wire = lazy.wire();
            assert!(wire.compressed);
            assert_eq!(lazy.rescale(), Some((from, coords)));
            assert_eq!(wire.bytes, payload);
        }
        _ => panic!("expected LazyMvt variant"),
    }
}

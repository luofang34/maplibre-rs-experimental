#![allow(clippy::expect_used, clippy::panic)]

use super::super::bytes_cache::TileBytesCache;
use crate::{
    coords::{WorldTileCoords, ZoomLevel},
    render::ShaderVertex,
    tcs::tiles::Tiles,
    vector::{
        tessellation::OverAlignedVertexBuffer, AvailableVectorLayerBucket, VectorLayerBucket,
        VectorLayerBucketComponent,
    },
};

#[test]
fn cached_tile_bytes_invalidates_when_cpu_vector_geometry_is_released() {
    let coords = WorldTileCoords::from((4, 5, ZoomLevel::new(4)));
    let mut tiles = Tiles::default();
    tiles
        .spawn_mut(coords)
        .expect("coords")
        .insert(VectorLayerBucketComponent {
            failed: false,
            done: true,
            overscaled_zoom: 0,
            layers: vec![VectorLayerBucket::AvailableLayer(
                AvailableVectorLayerBucket {
                    coords,
                    source_layer: "water".to_owned(),
                    style_layer_id: "water".to_owned(),
                    buffer: OverAlignedVertexBuffer::from_iters(
                        vec![ShaderVertex::new([0.0, 0.0], [0.0, 0.0]); 128],
                        vec![0_u32; 128],
                        128,
                    ),
                    feature_indices: Vec::new(),
                    feature_colors: Vec::new(),
                    feature_sort_keys: Vec::new(),
                },
            )],
            uploaded_stamp: None,
        });

    let mut cache = TileBytesCache::default();
    let before = cache.get_or_compute(&tiles, coords);
    assert!(before > 0);
    assert_eq!(cache.len(), 1);
    assert_eq!(cache.get_or_compute(&tiles, coords), before);

    // Mutate the CPU buffer in place (simulating CPU release after GPU upload).
    if let Some(buckets) = tiles.query_mut::<&mut VectorLayerBucketComponent>(coords) {
        if let VectorLayerBucket::AvailableLayer(layer) = &mut buckets.layers[0] {
            layer.buffer.buffer.vertices.clear();
            layer.buffer.buffer.vertices.shrink_to_fit();
            layer.buffer.buffer.indices.clear();
            layer.buffer.buffer.indices.shrink_to_fit();
        }
    }
    let after = cache.get_or_compute(&tiles, coords);
    assert!(
        after < before,
        "tile_bytes cache must invalidate when CPU buffers are released ({before} -> {after})"
    );
}

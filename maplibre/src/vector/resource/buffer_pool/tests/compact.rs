#![cfg(all(feature = "headless", feature = "thread-safe-futures"))]

use super::*;
use crate::render::{
    shaders::{
        EmptyLayerMetadata, FillShaderFeatureMetadata, GpuShaderVertex, ShaderLayerMetadata,
        ShaderSymbolVertex,
    },
    ShaderVertex,
};

#[tokio::test]
async fn gpu_pool_uses_u16_indices_and_spills_gt_65k_buckets_to_u32_without_dropping() {
    use crate::{headless::create_headless_renderer, vector::VectorBufferPool};

    let (_, renderer) = create_headless_renderer(32, 32, None)
        .await
        .expect("renderer");
    let mut pool = VectorBufferPool::from_device(&renderer.device)
        .with_decoupled_feature_metadata()
        .with_shared_layer_metadata();

    let small_coords = WorldTileCoords::from((0, 0, 13_u8.into()));
    let small_geom =
        OverAlignedVertexBuffer::from_iters(vec![ShaderVertex::default(); 100], [0_u32, 1, 2], 3);
    pool.replace_layer_geometry_layer_uniform_shared_unless(
        &renderer.queue,
        small_coords,
        UniformLayerUpload {
            style_layer: std::sync::Arc::new(style_layer("building")),
            geometry: &small_geom,
            layer_metadata: ShaderLayerMetadata::new(0.0, 1.0, [0.0; 2]),
            uniform_feature: FillShaderFeatureMetadata::default(),
            records: 100,
        },
        None,
        |_| false,
    )
    .expect("small bucket uploads into u16 index slab");

    let small_entry = &pool.index().get_layers(small_coords).expect("small")[0];
    assert!(small_entry.is_u16_indices());
    assert_eq!(
        (small_entry.vertices_buffer_range().end - small_entry.vertices_buffer_range().start)
            as usize,
        100 * std::mem::size_of::<GpuShaderVertex>()
    );
    assert_eq!(
        small_entry.indices_buffer_range().end - small_entry.indices_buffer_range().start,
        8,
        "3 u16 indices pad to 4 u16s (8 B) for COPY_BUFFER_ALIGNMENT"
    );
    assert!(
        pool.index_native_slabs.is_empty(),
        "no u32 fallback index slab allocated when all buckets fit u16"
    );
    assert!(
        pool.feature_slabs.is_empty()
            && pool
                .vertex_slabs
                .iter()
                .flatten()
                .all(|s| s.paired_fm.is_none()),
        "uniform layer-metadata uploads allocate zero feature_metadata buffers"
    );

    // Dense Tokyo z14 building bucket with 70,000 vertices (> 65,536) must automatically use u32!
    let dense_coords = WorldTileCoords::from((1, 0, 14_u8.into()));
    let dense_geom = OverAlignedVertexBuffer::from_iters(
        vec![ShaderVertex::default(); 70_000],
        [0_u32, 65_535, 69_999],
        3,
    );
    pool.replace_layer_geometry_layer_uniform_shared_unless(
        &renderer.queue,
        dense_coords,
        UniformLayerUpload {
            style_layer: std::sync::Arc::new(style_layer("building")),
            geometry: &dense_geom,
            layer_metadata: ShaderLayerMetadata::new(0.0, 1.0, [0.0; 2]),
            uniform_feature: FillShaderFeatureMetadata::default(),
            records: 70_000,
        },
        None,
        |_| false,
    )
    .expect("dense >65k vertex bucket falls back to u32 index slab cleanly");

    let dense_entry = &pool.index().get_layers(dense_coords).expect("dense")[0];
    assert!(!dense_entry.is_u16_indices());
    assert_eq!(dense_entry.usable_indices, 3);
    assert_eq!(
        pool.index_native_slabs.iter().flatten().count(),
        1,
        "exactly one on-demand u32 slab allocated for >65k bucket"
    );
}

#[cfg(all(feature = "headless", feature = "thread-safe-futures"))]
#[tokio::test]
async fn symbol_pool_with_empty_layer_metadata_allocates_zero_layer_and_standalone_fm_slabs() {
    use crate::{headless::create_headless_renderer, sdf::SymbolBufferPool};

    let (_, renderer) = create_headless_renderer(32, 32, None)
        .await
        .expect("renderer");
    let mut sym_pool = SymbolBufferPool::from_device(&renderer.device);
    let coords = WorldTileCoords::from((0, 0, 10_u8.into()));
    let geom = OverAlignedVertexBuffer::from_iters(
        [ShaderSymbolVertex::default(); 8],
        [0_u32, 1, 2, 0, 2, 3],
        6,
    );
    sym_pool
        .replace_layer_geometry_unless(
            &renderer.queue,
            coords,
            LayerGeometryUpload {
                style_layer: std::sync::Arc::new(style_layer("place-label")),
                geometry: &geom,
                layer_metadata: EmptyLayerMetadata([0; 0]),
                feature_metadata: &[],
            },
            |_| false,
        )
        .expect("symbol upload");

    let entry = &sym_pool.index().get_layers(coords).expect("sym")[0];
    assert!(entry.is_u16_indices());
    assert!(sym_pool.layer_slabs.is_empty());
    assert!(sym_pool.feature_slabs.is_empty());
    assert!(sym_pool.index_native_slabs.is_empty());
}

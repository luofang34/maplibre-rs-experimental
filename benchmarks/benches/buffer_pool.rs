#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion};
use maplibre::{
    coords::{WorldTileCoords, ZoomLevel},
    headless::create_headless_renderer,
    platform::run_multithreaded,
    render::{
        resource::Queue,
        settings::BufferPoolSizes,
        shaders::{FillShaderFeatureMetadata, ShaderLayerMetadata},
        ShaderVertex,
    },
    sdf::SymbolBufferPool,
    style::layer::StyleLayer,
    vector::{tessellation::OverAlignedVertexBuffer, BufferPool, VectorBufferPool},
};

#[derive(Debug)]
struct BenchBuffer {
    size: wgpu::BufferAddress,
}

struct BenchQueue;

impl Queue<BenchBuffer> for BenchQueue {
    fn write_buffer(&self, buffer: &BenchBuffer, offset: wgpu::BufferAddress, data: &[u8]) {
        assert!(offset + data.len() as wgpu::BufferAddress <= buffer.size);
    }
}

fn bench_buffer_pool_churn_and_coalesce(c: &mut Criterion) {
    let layer: StyleLayer = serde_json::from_value(serde_json::json!({
        "id": "buildings",
        "type": "fill",
        "source": "openmaptiles",
        "source-layer": "building"
    }))
    .expect("valid style layer");
    let geom = OverAlignedVertexBuffer::from_iters(
        [ShaderVertex::default(); 64],
        (0..96_u32).map(|i| i % 64),
        96,
    );
    let layer_meta = ShaderLayerMetadata::new(0.0, 1.0, [0.0; 2]);
    let feature_meta = vec![
        FillShaderFeatureMetadata {
            color: [0.5, 0.5, 0.5, 1.0]
        };
        64
    ];

    c.bench_function("buffer_pool_mid_eviction_churn_256_tiles", |b| {
        b.iter(|| {
            let mut pool: BufferPool<
                BenchQueue,
                BenchBuffer,
                ShaderVertex,
                u32,
                ShaderLayerMetadata,
                FillShaderFeatureMetadata,
            > = BufferPool::with_page_factory(
                BufferPoolSizes::default(),
                256 << 20,
                |_typ, bytes| BenchBuffer {
                    size: bytes.max(16),
                },
            );

            for i in 0..256_i32 {
                let coords = WorldTileCoords {
                    x: i % 16,
                    y: i / 16,
                    z: ZoomLevel::new(12),
                };
                pool.allocate_layer_geometry(
                    &BenchQueue,
                    coords,
                    layer.clone(),
                    &geom,
                    layer_meta,
                    &feature_meta,
                )
                .expect("allocate");
            }
            // Evict every even tile in the middle and immediately reuse coalesced holes.
            for i in (0..256_i32).step_by(2) {
                let coords = WorldTileCoords {
                    x: i % 16,
                    y: i / 16,
                    z: ZoomLevel::new(12),
                };
                pool.remove_tile(coords);
            }
            for i in (0..256_i32).step_by(2) {
                let coords = WorldTileCoords {
                    x: (i % 16) + 16,
                    y: i / 16,
                    z: ZoomLevel::new(12),
                };
                pool.allocate_layer_geometry(
                    &BenchQueue,
                    coords,
                    layer.clone(),
                    &geom,
                    layer_meta,
                    &feature_meta,
                )
                .expect("reuse coalesced hole");
                black_box(pool.index().max_zoom());
            }
        });
    });
}

fn bench_gpu_pool_startup_and_first_tile(c: &mut Criterion) {
    let (_, renderer) = run_multithreaded(async {
        create_headless_renderer(64, 64, None)
            .await
            .expect("benchmark renderer")
    });
    let layer: StyleLayer = serde_json::from_value(serde_json::json!({
        "id": "water",
        "type": "fill",
        "source": "openmaptiles",
        "source-layer": "water"
    }))
    .expect("valid style layer");
    let geom = OverAlignedVertexBuffer::from_iters(
        [ShaderVertex::default(); 128],
        (0..192_u32).map(|i| i % 128),
        192,
    );
    let layer_meta = ShaderLayerMetadata::new(0.0, 1.0, [0.0; 2]);
    let feature_meta = vec![
        FillShaderFeatureMetadata {
            color: [0.0, 0.4, 0.8, 1.0]
        };
        128
    ];

    c.bench_function("gpu_pool_init_both_and_upload_first_tile", |b| {
        b.iter(|| {
            let mut vec_pool = VectorBufferPool::from_device(&renderer.device);
            let sym_pool = SymbolBufferPool::from_device(&renderer.device);
            vec_pool
                .allocate_layer_geometry(
                    &renderer.queue,
                    WorldTileCoords::default(),
                    layer.clone(),
                    &geom,
                    layer_meta,
                    &feature_meta,
                )
                .expect("upload");
            black_box((
                vec_pool.device_buffer_bytes(),
                sym_pool.device_buffer_bytes(),
            ));
        });
    });
}

criterion_group!(
    name = benches;
    config = Criterion::default().sample_size(20);
    targets = bench_buffer_pool_churn_and_coalesce, bench_gpu_pool_startup_and_first_tile
);
criterion_main!(benches);

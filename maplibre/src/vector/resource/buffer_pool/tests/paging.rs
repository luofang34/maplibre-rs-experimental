use std::{cell::Cell, rc::Rc};

use super::*;
use crate::render::settings::BufferPoolSizes;

#[test]
fn mid_page_hole_coalesces_immediately_without_ring_wrap() {
    // 5 vertices capacity (120 usable bytes of 24-byte TestVertex).
    let mut pool = BufferPool::new(
        BackingBufferDescriptor::new(TestBuffer { size: 120 }, 120),
        BackingBufferDescriptor::new(TestBuffer { size: 128 }, 128),
        BackingBufferDescriptor::new(TestBuffer { size: 128 }, 128),
        BackingBufferDescriptor::new(TestBuffer { size: 128 }, 128),
    );
    let a = WorldTileCoords::from((0, 0, 2_u8.into()));
    let b = WorldTileCoords::from((1, 0, 2_u8.into()));
    let c = WorldTileCoords::from((2, 0, 2_u8.into()));
    let d = WorldTileCoords::from((3, 0, 2_u8.into()));
    let e = WorldTileCoords::from((0, 1, 2_u8.into()));

    // Fill all 5 vertices: a(1), b(2), c(1), d(1).
    for (coords, count) in [(a, 1), (b, 2), (c, 1), (d, 1)] {
        pool.allocate_layer_geometry(
            &TestQueue,
            coords,
            style_layer("land"),
            &geometry(count),
            0,
            &vec![0; count],
        )
        .expect("initial fill");
    }
    assert_eq!(pool.available_space(BackingBufferType::Vertices), 0);

    // Evict middle tiles `b` (2) and `c` (1): they must coalesce into a contiguous 3-vertex
    // (72-byte) block while `a` (oldest) and `d` (newest) remain protected!
    pool.remove_tile(b);
    pool.remove_tile(c);
    assert_eq!(pool.available_space(BackingBufferType::Vertices), 72);

    // Allocate `e` needing 3 contiguous vertices while protecting `a` and `d`.
    // A naive ring index would fail here because `a` sits at 0..24 and `d` sits at 96..120!
    pool.allocate_layer_geometry_unless(
        &TestQueue,
        e,
        style_layer("land"),
        &geometry(3),
        0,
        &[0; 3],
        |tile| tile == a || tile == d,
    )
    .expect("coalesced mid-buffer hole reused without evicting protected boundary tiles");

    assert!(pool.has_layer(a, "land"));
    assert!(pool.has_layer(d, "land"));
    assert!(pool.has_layer(e, "land"));
}

#[test]
fn paged_pool_starts_at_zero_grows_across_pages_and_reclaims_burst_pages() {
    let created_bytes = Rc::new(Cell::new(0_u64));
    let created_clone = created_bytes.clone();

    // Total ceiling: 12 vertices (288 B), max single buffer page: 96 B (4 vertices per page).
    let mut pool: BufferPool<TestQueue, TestBuffer, TestVertex, u32, u32, u32> =
        BufferPool::with_page_factory(
            BufferPoolSizes {
                vertices: 12,
                indices: 12,
                feature_metadata: 12,
                layer_metadata: 12,
            },
            96,
            move |_typ, bytes| {
                created_clone.set(created_clone.get() + bytes);
                TestBuffer {
                    size: bytes.max(16),
                }
            },
        );

    assert!(!pool.is_empty());
    assert!(pool.has_capacity());
    assert_eq!(
        pool.page_count(),
        0,
        "no pages allocated before first upload"
    );
    assert_eq!(created_bytes.get(), 0);

    let tiles: Vec<_> = (0..3)
        .map(|x| WorldTileCoords::from((x, 0, 2_u8.into())))
        .collect();

    // Upload 3 tiles of 4 vertices each -> spans 3 pages (0, 1, 2).
    for &coords in &tiles {
        pool.allocate_layer_geometry(
            &TestQueue,
            coords,
            style_layer("buildings"),
            &geometry(4),
            0,
            &[0; 4],
        )
        .expect("page growth");
    }
    assert_eq!(pool.page_count(), 3);
    assert_eq!(pool.index().max_zoom(), Some(2));

    // Removing 2 tiles leaves 2 empty pages; automatic hysteresis keeps 1 warm spare page and
    // drops the extra empty burst page immediately (3 -> 2 pages: 1 live + 1 warm spare).
    pool.remove_tile(tiles[1]);
    pool.remove_tile(tiles[2]);
    assert_eq!(pool.page_count(), 2);

    // Removing the last tile leaves 2 empty pages -> collapses to 1 warm spare page.
    pool.remove_tile(tiles[0]);
    assert_eq!(pool.page_count(), 1);
    assert_eq!(pool.index().max_zoom(), None);

    // Tight memory pressure reclaims even the last idle page down to 0 pages!
    pool.reclaim_empty_pages(0);
    assert_eq!(pool.page_count(), 0);
}

#[cfg(all(feature = "headless", feature = "thread-safe-futures"))]
#[tokio::test]
async fn default_gpu_pool_starts_at_zero_bytes_and_allocates_one_page_on_demand() {
    use crate::{
        headless::create_headless_renderer,
        render::{
            shaders::{FillShaderFeatureMetadata, ShaderLayerMetadata},
            ShaderVertex,
        },
        vector::VectorBufferPool,
    };

    let (_, renderer) = create_headless_renderer(32, 32, None)
        .await
        .expect("renderer");
    let mut pool = VectorBufferPool::from_device(&renderer.device);
    assert!(pool.has_capacity());
    assert!(!pool.is_empty());
    assert_eq!(
        pool.device_buffer_bytes(),
        0,
        "default VectorBufferPool must allocate 0 GPU bytes before first upload"
    );

    let coords = WorldTileCoords::from((0, 0, 1_u8.into()));
    let geom = OverAlignedVertexBuffer::from_iters(
        [ShaderVertex::default(); 4],
        [0_u32, 1, 2, 2, 1, 3],
        6,
    );
    pool.allocate_layer_geometry(
        &renderer.queue,
        coords,
        style_layer("water"),
        &geom,
        ShaderLayerMetadata::new(0.0, 1.0, [0.0; 2]),
        &[FillShaderFeatureMetadata {
            color: [0.0, 0.5, 1.0, 1.0],
        }; 4],
    )
    .expect("first GPU page upload");

    let one_page_bytes = pool.device_buffer_bytes();
    assert_eq!(pool.page_count(), 1);
    assert!(
        one_page_bytes > 0 && one_page_bytes <= 16 * 1024 * 1024,
        "expected single ~10 MiB on-demand page instead of ~347 MiB monolith, got {one_page_bytes} B"
    );
}

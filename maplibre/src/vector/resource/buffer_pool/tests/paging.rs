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
        &[FillShaderFeatureMetadata::from_rgba([0.0, 0.5, 1.0, 1.0]); 4],
    )
    .expect("first GPU page upload");

    let one_page_bytes = pool.device_buffer_bytes();
    assert_eq!(pool.page_count(), 1);
    assert!(
        one_page_bytes > 0 && one_page_bytes <= 16 * 1024 * 1024,
        "expected single ~10 MiB on-demand page instead of ~347 MiB monolith, got {one_page_bytes} B"
    );
}

#[test]
fn decoupled_pool_shares_vertex_allocation_and_uses_single_uniform_metadata_record() {
    #[repr(C)]
    #[derive(Default, Copy, Clone, bytemuck_derive::Pod, bytemuck_derive::Zeroable)]
    struct UniformMeta {
        rgba: [f32; 4],
    }

    let mut pool: BufferPool<TestQueue, TestBuffer, TestVertex, u32, u32, UniformMeta> =
        BufferPool::new(
            BackingBufferDescriptor::new(TestBuffer { size: 256 }, 256),
            BackingBufferDescriptor::new(TestBuffer { size: 256 }, 256),
            BackingBufferDescriptor::new(TestBuffer { size: 256 }, 256),
            BackingBufferDescriptor::new(TestBuffer { size: 256 }, 256),
        )
        .with_decoupled_feature_metadata();
    assert!(pool.is_decoupled_feature_metadata());

    let coords = WorldTileCoords::from((1, 1, 2_u8.into()));
    let primary_geom = geometry(6).with_shared_geometry_key(Some(7));
    let mut secondary_buf = VertexBuffers::new();
    secondary_buf.indices = vec![0, 1, 2];
    let secondary_geom =
        OverAlignedVertexBuffer::from(secondary_buf).with_shared_geometry_key(Some(7));

    pool.replace_layer_geometry_uniform_shared_unless(
        &TestQueue,
        coords,
        style_layer("road-casing"),
        &primary_geom,
        None,
        1_u32,
        UniformMeta {
            rgba: [0.1, 0.2, 0.3, 1.0],
        },
        6,
        |_| false,
    )
    .expect("primary shared upload");

    let free_after_primary = pool.available_space(BackingBufferType::Vertices);
    assert_eq!(free_after_primary, (256 / 24) * 24 - 6 * 24);

    pool.replace_layer_geometry_uniform_shared_unless(
        &TestQueue,
        coords,
        style_layer("road-core"),
        &secondary_geom,
        Some(&primary_geom.buffer.vertices),
        2_u32,
        UniformMeta {
            rgba: [0.9, 0.8, 0.1, 1.0],
        },
        6,
        |_| false,
    )
    .expect("secondary shared upload");

    assert_eq!(
        pool.available_space(BackingBufferType::Vertices),
        free_after_primary,
        "second sibling must consume 0 additional vertex bytes"
    );

    let entries = pool.index().get_layers(coords).expect("entries");
    assert_eq!(entries.len(), 2);
    let casing = entries
        .iter()
        .find(|e| e.style_layer.id == "road-casing")
        .expect("casing");
    let core = entries
        .iter()
        .find(|e| e.style_layer.id == "road-core")
        .expect("core");
    assert!(casing.is_uniform_feature_metadata());
    assert!(core.is_uniform_feature_metadata());
    let casing_fm = casing.feature_metadata_buffer_range();
    let core_fm = core.feature_metadata_buffer_range();
    assert_eq!(casing_fm.end - casing_fm.start, 16);
    assert_eq!(core_fm.end - core_fm.start, 16);
    assert_ne!(casing_fm, core_fm);
    assert_eq!(casing.vertices_buffer_range(), core.vertices_buffer_range());
    assert_eq!(casing.base_vertex(), core.base_vertex());

    // Removing the first sibling retains the shared vertex slab for the remaining sibling.
    pool.remove_layer(coords, "road-casing");
    assert_eq!(
        pool.available_space(BackingBufferType::Vertices),
        free_after_primary
    );

    // Removing the last sibling releases the shared vertex slab back to the page allocator.
    pool.remove_layer(coords, "road-core");
    assert_eq!(
        pool.available_space(BackingBufferType::Vertices),
        (256 / 24) * 24
    );
}

#[test]
fn shared_layer_metadata_reuses_one_slot_across_tiles_and_reserves_zero_feature_bytes() {
    #[repr(C)]
    #[derive(
        Default, Copy, Clone, Debug, PartialEq, bytemuck_derive::Pod, bytemuck_derive::Zeroable,
    )]
    struct LayerMeta64 {
        words: [u32; 16],
    }

    let mut pool: BufferPool<TestQueue, TestBuffer, TestVertex, u32, LayerMeta64, u32> =
        BufferPool::new(
            BackingBufferDescriptor::new(TestBuffer { size: 512 }, 512),
            BackingBufferDescriptor::new(TestBuffer { size: 512 }, 512),
            BackingBufferDescriptor::new(TestBuffer { size: 256 }, 256),
            BackingBufferDescriptor::new(TestBuffer { size: 256 }, 256),
        )
        .with_decoupled_feature_metadata()
        .with_shared_layer_metadata();

    let t0 = WorldTileCoords::from((0, 0, 4_u8.into()));
    let t1 = WorldTileCoords::from((1, 0, 4_u8.into()));
    let meta = LayerMeta64 { words: [7; 16] };

    pool.replace_layer_geometry_layer_uniform_shared_unless(
        &TestQueue,
        t0,
        style_layer("roads"),
        &geometry(3),
        None,
        meta,
        0_u32,
        3,
        |_| false,
    )
    .expect("first tile");
    let lm_free_after_first = pool.available_space(BackingBufferType::Metadata);
    assert_eq!(lm_free_after_first, 256 - 64);
    assert_eq!(
        pool.available_space(BackingBufferType::FeatureMetadata),
        256,
        "layer-uniform fill/line must reserve 0 feature-metadata bytes"
    );

    pool.replace_layer_geometry_layer_uniform_shared_unless(
        &TestQueue,
        t1,
        style_layer("roads"),
        &geometry(3),
        None,
        meta,
        0_u32,
        3,
        |_| false,
    )
    .expect("second tile at same zoom shares layer metadata");

    assert_eq!(
        pool.available_space(BackingBufferType::Metadata),
        lm_free_after_first,
        "second tile of same style layer and zoom must share the 64-byte layer metadata slot"
    );
    assert_eq!(
        pool.available_space(BackingBufferType::FeatureMetadata),
        256
    );

    let r0 = pool.index().get_layers(t0).expect("t0")[0].layer_metadata_buffer_range();
    let r1 = pool.index().get_layers(t1).expect("t1")[0].layer_metadata_buffer_range();
    assert_eq!(r0, r1);
    assert!(pool.index().get_layers(t0).expect("t0")[0]
        .feature_metadata_buffer_range()
        .is_empty());

    pool.remove_tile(t0);
    assert_eq!(
        pool.available_space(BackingBufferType::Metadata),
        lm_free_after_first,
        "shared layer metadata stays allocated while t1 still references it"
    );

    pool.remove_tile(t1);
    assert_eq!(
        pool.available_space(BackingBufferType::Metadata),
        256,
        "shared layer metadata is freed once the last tile on the page is removed"
    );
}

#[test]
fn eviction_skips_protected_front_tile_and_reclaims_unprotected_tail_tiles() {
    let mut pool = BufferPool::new(
        BackingBufferDescriptor::new(TestBuffer { size: 96 }, 96),
        BackingBufferDescriptor::new(TestBuffer { size: 128 }, 128),
        BackingBufferDescriptor::new(TestBuffer { size: 128 }, 128),
        BackingBufferDescriptor::new(TestBuffer { size: 128 }, 128),
    );
    let front_protected = WorldTileCoords::from((0, 0, 3_u8.into()));
    let stale_behind = WorldTileCoords::from((1, 0, 3_u8.into()));
    let incoming_visible = WorldTileCoords::from((2, 0, 3_u8.into()));

    // Upload `front_protected` (2 vertices) first, then `stale_behind` (2 vertices), filling 4/4.
    for coords in [front_protected, stale_behind] {
        pool.allocate_layer_geometry(
            &TestQueue,
            coords,
            style_layer("buildings"),
            &geometry(2),
            0,
            &[0; 2],
        )
        .expect("initial fill");
    }
    assert_eq!(pool.available_space(BackingBufferType::Vertices), 0);

    // `front_protected` is at `index.front()` and remains visible; `stale_behind` left the view.
    pool.allocate_layer_geometry_unless(
        &TestQueue,
        incoming_visible,
        style_layer("buildings"),
        &geometry(2),
        0,
        &[0; 2],
        |tile| tile == front_protected || tile == incoming_visible,
    )
    .expect("eviction must skip protected front tile and reclaim unprotected tile behind it");

    assert!(pool.has_layer(front_protected, "buildings"));
    assert!(!pool.has_layer(stale_behind, "buildings"));
    assert!(pool.has_layer(incoming_visible, "buildings"));
}

#[test]
fn paged_pool_utilizes_vertex_budget_without_layer_metadata_starvation() {
    let mut pool: BufferPool<TestQueue, TestBuffer, TestVertex, u32, [u32; 16], u32> =
        BufferPool::with_page_factory(
            BufferPoolSizes {
                vertices: 6_000_000,
                indices: 18_000_000,
                feature_metadata: 5_000_000,
                layer_metadata: 32_768,
            },
            64 * 1024 * 24,
            move |_typ, bytes| TestBuffer {
                size: bytes.max(16),
            },
        )
        .with_decoupled_feature_metadata()
        .with_shared_layer_metadata();

    let dense_geom = geometry(60_000);
    // 60 tiles * 60_000 vertices = 3.6M vertices across 60 `64K`-capped pages (> 16 pages).
    for x in 0..60 {
        let coords = WorldTileCoords::from((x, 0, 13_u8.into()));
        pool.replace_layer_geometry_layer_uniform_shared_unless(
            &TestQueue,
            coords,
            style_layer("building"),
            &dense_geom,
            None,
            [0_u32; 16],
            0_u32,
            60_000,
            |_| true,
        )
        .expect("layer_metadata page sizing must not starve multi-page vertex capacity");
    }
    assert!(pool.page_count() >= 50);
}

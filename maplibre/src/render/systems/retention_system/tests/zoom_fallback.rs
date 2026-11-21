#![allow(clippy::expect_used, clippy::panic)]

use std::collections::HashSet;

use super::spawn_vector;
use crate::{
    coords::{WorldTileCoords, ZoomLevel},
    io::tile_retry::{self, RequestKind},
    tcs::world::World,
    vector::{
        content::LayerReplacements, feature_paint::TileFeaturePaintComponent,
        tessellation::OverAlignedVertexBuffer, AvailableVectorLayerBucket, VectorLayerBucket,
        VectorLayerBucketComponent,
    },
};

#[cfg(all(feature = "headless", feature = "thread-safe-futures"))]
#[tokio::test]
async fn evict_before_upload_protects_drawn_and_extra_in_use_tiles_across_zoom_change() {
    use super::insert_gpu_layer;
    use crate::{
        coords::{LatLon, ViewRegion, WorldCoords, Zoom},
        headless::create_headless_renderer,
        io::tile_sources::TileKind,
        render::{
            eventually::Eventually,
            memory_budget::MemoryBudget,
            resource::BackingBufferDescriptor,
            settings::BufferPoolSizes,
            tile_view_pattern::{ViewTileSources, WgpuTileViewPattern},
            view_state::ViewState,
        },
        vector::{VectorBufferPool, VectorTilesDone},
        window::PhysicalSize,
    };

    let (_, renderer) = create_headless_renderer(64, 64, None)
        .await
        .expect("renderer");
    let sizes = BufferPoolSizes {
        vertices: 64,
        indices: 64,
        feature_metadata: 64,
        layer_metadata: 8,
    };
    let mut vectors = VectorBufferPool::from_device_with_sizes(&renderer.device, sizes);
    let mut world = World::default();
    let drawn = WorldTileCoords {
        x: 2048,
        y: 2048,
        z: ZoomLevel::new(12),
    };
    let painted = WorldTileCoords {
        x: 512,
        y: 512,
        z: ZoomLevel::new(10),
    };
    let stale = WorldTileCoords {
        x: 0,
        y: 0,
        z: ZoomLevel::new(4),
    };
    for coords in [drawn, painted, stale] {
        spawn_vector(&mut world, coords, true);
        insert_gpu_layer(&mut vectors, &renderer.queue, coords);
    }

    world.resources.insert(Eventually::Initialized(vectors));
    world
        .resources
        .get_or_init_mut::<ViewTileSources>()
        .add::<VectorTilesDone>(TileKind::Vector);

    let pattern_buffer = renderer.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("pattern"),
        size: 4096,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut pattern = WgpuTileViewPattern::new(BackingBufferDescriptor::new(pattern_buffer, 4096));
    let region = ViewRegion::from_tiles(vec![drawn], ZoomLevel::new(12), 1);
    let view_tiles = {
        let sources = world.resources.get::<ViewTileSources>().expect("sources");
        pattern.generate_pattern(&region, sources, &[], Zoom::new(12.0), &world)
    };
    assert_eq!(view_tiles.len(), 1);
    pattern.update_pattern(view_tiles);

    world.resources.insert(Eventually::Initialized(pattern));
    // Force cache budget = 0 (`MemoryPressure::Critical`) so only `in_use` survives.
    world.resources.insert(MemoryBudget {
        available_bytes: Some(0),
    });

    let style = crate::style::Style::default();
    let view_state = ViewState::new(
        PhysicalSize::new(512, 512).expect("size"),
        WorldCoords::from_lat_lon(LatLon::new(35.6812, 139.7671), Zoom::new(14.5)),
        Zoom::new(14.5),
        cgmath::Deg(0.0),
        cgmath::Deg(45.0),
    );
    super::super::evict_before_upload(
        &mut world,
        &style,
        &view_state,
        &renderer.queue,
        true,
        &[painted],
    );

    assert!(world.tiles.exists(drawn), "currently drawn tile must stay");
    assert!(
        world.tiles.exists(painted),
        "extra_in_use stand-in tile must stay across zoom crossing"
    );
    assert!(
        !world.tiles.exists(stale),
        "unrelated tile outside in_use must be evicted under zero cache budget"
    );
    let Some(Eventually::Initialized(vectors)) =
        world.resources.get::<Eventually<VectorBufferPool>>()
    else {
        panic!("vector pool");
    };
    assert!(vectors.get_loaded_style_layers_at(drawn).is_some());
    assert!(vectors.get_loaded_style_layers_at(painted).is_some());
    assert!(vectors.get_loaded_style_layers_at(stale).is_none());
}

#[test]
fn drop_cancelled_keeps_committed_resident_tiles_and_drops_uncommitted_tiles() {
    let mut world = World::default();
    let resident = WorldTileCoords {
        x: 100,
        y: 100,
        z: ZoomLevel::new(8),
    };
    let uncommitted = WorldTileCoords {
        x: 101,
        y: 100,
        z: ZoomLevel::new(8),
    };

    let make_bucket = || AvailableVectorLayerBucket {
        coords: resident,
        source_layer: "water".into(),
        style_layer_id: "water".into(),
        buffer: OverAlignedVertexBuffer::empty(),
        feature_indices: Vec::new(),
        feature_colors: Vec::new(),
        feature_sort_keys: Vec::new(),
    };
    world
        .tiles
        .spawn_mut(resident)
        .expect("resident")
        .insert(VectorLayerBucketComponent {
            failed: false,
            done: true,
            overscaled_zoom: 0,
            layers: vec![VectorLayerBucket::AvailableLayer(make_bucket())],
            uploaded_stamp: None,
        })
        .insert(LayerReplacements {
            vector: vec![make_bucket()],
            keep_vector: true,
            requested: vec!["water".into()],
            delivered: vec!["water".into()],
            ..Default::default()
        })
        .insert(TileFeaturePaintComponent {
            pending: std::iter::once(("water".to_owned(), Vec::new())).collect(),
            ..Default::default()
        });
    spawn_vector(&mut world, uncommitted, false);

    for coords in [resident, uncommitted] {
        let attempt = tile_retry::next_attempt(&mut world);
        tile_retry::started(&mut world, coords, RequestKind::Vector, attempt, None);
    }
    tile_retry::want(&mut world, RequestKind::Vector, &HashSet::new());

    super::super::drop_cancelled(&mut world, &HashSet::new(), None);

    assert!(
        world.tiles.exists(resident),
        "committed resident tile must survive request cancellation"
    );
    let pending = world
        .tiles
        .query::<&LayerReplacements>(resident)
        .expect("replacements");
    assert!(pending.vector.is_empty());
    assert!(pending.requested.is_empty());
    assert!(!pending.keep_vector);
    let paint_store = world
        .tiles
        .query::<&TileFeaturePaintComponent>(resident)
        .expect("paint store");
    assert!(paint_store.pending.is_empty());

    assert!(
        !world.tiles.exists(uncommitted),
        "uncommitted cancelled tile must still be dropped"
    );
}

#[cfg(all(feature = "headless", feature = "thread-safe-futures"))]
#[tokio::test]
async fn reclaim_pool_pages_keeps_spare_only_when_comfortable() {
    use super::insert_gpu_layer;
    use crate::{
        headless::create_headless_renderer,
        render::{
            eventually::Eventually,
            memory_budget::{MemoryBudget, LOW_MEMORY_BYTES},
            settings::BufferPoolSizes,
        },
        vector::VectorBufferPool,
    };

    let (_, renderer) = create_headless_renderer(64, 64, None)
        .await
        .expect("renderer");
    let sizes = BufferPoolSizes {
        vertices: 8,
        indices: 16,
        feature_metadata: 8,
        layer_metadata: 2,
    };
    let mut vectors = VectorBufferPool::from_device_with_sizes(&renderer.device, sizes);
    let coords = WorldTileCoords::default();
    insert_gpu_layer(&mut vectors, &renderer.queue, coords);
    assert_eq!(vectors.page_count(), 1);
    vectors.remove_evicted_tile_with_queue(coords, None);
    assert_eq!(vectors.page_count(), 1);

    let mut world = World::default();
    world.resources.insert(Eventually::Initialized(vectors));
    world.resources.insert(MemoryBudget::default());
    super::super::gpu::reclaim_pool_pages(&mut world, None);

    let Some(Eventually::Initialized(vectors)) =
        world.resources.get::<Eventually<VectorBufferPool>>()
    else {
        panic!("vector pool");
    };
    assert_eq!(
        vectors.page_count(),
        1,
        "Comfortable budget retains 1 spare empty slab across zoom crossings"
    );

    world.resources.insert(MemoryBudget {
        available_bytes: Some(LOW_MEMORY_BYTES - 1),
    });
    super::super::gpu::reclaim_pool_pages(&mut world, None);
    let Some(Eventually::Initialized(vectors)) =
        world.resources.get::<Eventually<VectorBufferPool>>()
    else {
        panic!("vector pool");
    };
    assert_eq!(
        vectors.page_count(),
        0,
        "Tight memory pressure reclaims even the last spare slab"
    );

    // Also re-check Critical directly drops any empty spare.
    let mut vectors_crit = VectorBufferPool::from_device_with_sizes(&renderer.device, sizes);
    insert_gpu_layer(&mut vectors_crit, &renderer.queue, coords);
    vectors_crit.remove_evicted_tile_with_queue(coords, None);
    world
        .resources
        .insert(Eventually::Initialized(vectors_crit));
    world.resources.insert(MemoryBudget {
        available_bytes: Some(0),
    });
    super::super::gpu::reclaim_pool_pages(&mut world, None);
    let Some(Eventually::Initialized(vectors_crit)) =
        world.resources.get::<Eventually<VectorBufferPool>>()
    else {
        panic!("vector pool");
    };
    assert_eq!(vectors_crit.page_count(), 0);
}

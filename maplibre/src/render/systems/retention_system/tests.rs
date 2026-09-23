#![allow(clippy::expect_used, clippy::panic)]

use std::collections::HashSet;

use super::{evict_beyond, evict_stale_tiles, CacheBudget, MIN_CACHE_TILES};

#[test]
fn offline_tiles_survive_a_view_with_no_loaded_coverage() {
    let mut world = World::default();
    for x in 0..100 {
        world
            .tiles
            .spawn_mut(WorldTileCoords::from((x, 0, ZoomLevel::new(8))))
            .expect("valid tile");
    }
    world.resources.insert(super::RetainLoadedTiles);
    assert!(evict_stale_tiles(&mut world, &HashSet::new(), 0).is_empty());
    assert_eq!(world.tiles.tiles.len(), 100);
}
use crate::{
    coords::{WorldTileCoords, ZoomLevel},
    tcs::world::World,
    terrain::DemTileComponent,
    vector::{VectorLayerBucket, VectorLayerBucketComponent},
};

fn tile(index: i32) -> WorldTileCoords {
    WorldTileCoords {
        x: index % 32,
        y: index / 32,
        z: ZoomLevel::new(5),
    }
}

fn spawn_vector(world: &mut World, coords: WorldTileCoords, done: bool) {
    world
        .tiles
        .spawn_mut(coords)
        .expect("valid coordinates")
        .insert(VectorLayerBucketComponent {
            done,
            layers: Vec::new(),
        });
}

#[test]
fn tiles_beyond_the_budget_are_evicted_but_loading_and_in_use_tiles_stay() {
    let mut world = World::default();
    let extra = 10;
    for index in 0..(MIN_CACHE_TILES + extra) as i32 {
        spawn_vector(&mut world, tile(index), true);
    }
    let loading = tile(500);
    spawn_vector(&mut world, loading, false);
    let in_use = tile(600);
    spawn_vector(&mut world, in_use, true);
    world
        .tiles
        .spawn_mut(tile(700))
        .expect("valid coordinates")
        .insert(DemTileComponent::Pending);

    let evicted = evict_stale_tiles(&mut world, &HashSet::from([in_use]), 1);

    assert_eq!(evicted.len(), extra);
    assert!(
        world.tiles.exists(loading),
        "a tile still loading is never evicted"
    );
    assert!(world.tiles.exists(in_use));
    assert!(
        world.tiles.exists(tile(700)),
        "a pending DEM tile is never evicted"
    );
    assert_eq!(
        world.tiles.tiles.len(),
        MIN_CACHE_TILES + 3,
        "the cache keeps exactly the budget plus the protected tiles"
    );
    for coords in evicted {
        assert!(!world.tiles.exists(coords));
    }
}

#[test]
fn recently_used_tiles_outlive_never_used_ones() {
    let mut world = World::default();
    let recent = tile(0);
    spawn_vector(&mut world, recent, true);
    assert!(evict_stale_tiles(&mut world, &HashSet::from([recent]), 1).is_empty());

    for index in 1..=(MIN_CACHE_TILES + 3) as i32 {
        spawn_vector(&mut world, tile(index), true);
    }
    let evicted = evict_stale_tiles(&mut world, &HashSet::new(), 0);

    assert_eq!(evicted.len(), 4);
    assert!(!evicted.contains(&recent));
    assert!(world.tiles.exists(recent));
}

#[test]
fn nothing_is_evicted_within_the_budget() {
    let mut world = World::default();
    for index in 0..8 {
        spawn_vector(&mut world, tile(index), true);
    }
    assert!(evict_stale_tiles(&mut world, &HashSet::new(), 0).is_empty());
    assert_eq!(world.tiles.tiles.len(), 8);
}

#[test]
fn tiles_requested_around_an_eye_are_kept() {
    use crate::{
        coords::TILE_SIZE,
        render::{projection::view_region_for_projection, view_state::ViewStatePadding},
    };
    let style: crate::style::Style = serde_json::from_str(
        r#"{"version":8,"sources":{"dem":{"type":"raster-dem","tiles":["https://dem.example/{z}/{x}/{y}.png"],"tileSize":256,"maxzoom":12,"encoding":"terrarium"}},"layers":[],"terrain":{"source":"dem","exaggeration":1}}"#,
    )
    .expect("a terrain style parses");
    let eyed = wide_eye_view();
    let mut world = World::default();
    let requested: Vec<WorldTileCoords> = view_region_for_projection(
        &style,
        &eyed,
        &world,
        eyed.zoom().zoom_level(TILE_SIZE),
        ViewStatePadding::Loose,
    )
    .expect("the covering succeeds")
    .expect("a frustum covering is explicit")
    .iter()
    .collect();
    for coords in &requested {
        spawn_vector(&mut world, *coords, true);
    }
    // Enough tiles from an earlier view to overflow the smallest cache.
    let stale: Vec<WorldTileCoords> = (0..(MIN_CACHE_TILES as i32 + 40))
        .map(|index| WorldTileCoords {
            x: index,
            y: 0,
            z: ZoomLevel::new(20),
        })
        .collect();
    for coords in &stale {
        spawn_vector(&mut world, *coords, true);
    }

    let in_use = super::tiles_in_use(&world, &style, &eyed);
    let evicted = evict_stale_tiles(&mut world, &in_use, 0);

    assert!(
        requested.iter().all(|coords| !evicted.contains(coords)),
        "{} of {} requested tiles were evicted and would be fetched again next frame",
        requested.iter().filter(|c| evicted.contains(c)).count(),
        requested.len()
    );
    assert_eq!(
        evicted.len(),
        40,
        "the tiles of the earlier view beyond the cache go, the requested ones do not"
    );
}

#[test]
fn the_cache_is_sized_from_drawn_tiles_rather_than_requested_ones() {
    let mut world = World::default();
    let requested: HashSet<WorldTileCoords> = (0..100).map(tile).collect();
    for index in 0..200 {
        spawn_vector(&mut world, tile(index), true);
    }

    // Two drawn tiles: the cache stays at its minimum even though a hundred are requested.
    let evicted = evict_stale_tiles(&mut world, &requested, 2);

    assert_eq!(evicted.len(), 100 - MIN_CACHE_TILES);
    assert!(requested.iter().all(|coords| world.tiles.exists(*coords)));
}

#[test]
fn the_cache_is_bounded_in_bytes_as_well_as_tiles() {
    use crate::{
        render::ShaderVertex,
        vector::{tessellation::OverAlignedVertexBuffer, AvailableVectorLayerBucket},
    };

    let mut world = World::default();
    let vertex_bytes = std::mem::size_of::<ShaderVertex>();
    for index in 0..10 {
        let coords = tile(index);
        world
            .tiles
            .spawn_mut(coords)
            .expect("valid coordinates")
            .insert(VectorLayerBucketComponent {
                done: true,
                layers: vec![VectorLayerBucket::AvailableLayer(
                    AvailableVectorLayerBucket {
                        coords,
                        source_layer: "water".to_string(),
                        style_layer_id: "water".to_string(),
                        buffer: OverAlignedVertexBuffer {
                            buffer: lyon::tessellation::VertexBuffers {
                                vertices: vec![ShaderVertex::new([0.0, 0.0], [0.0, 0.0]); 1000],
                                indices: Vec::new(),
                            },
                            usable_indices: 0,
                        },
                        feature_indices: Vec::new(),
                        feature_colors: Vec::new(),
                    },
                )],
            });
    }

    // Ten tiles are within the tile budget; three of them fill the byte budget.
    let budget = CacheBudget {
        tiles: 64,
        bytes: 3 * 1000 * vertex_bytes,
    };
    let evicted = evict_beyond(&mut world, &HashSet::new(), budget);

    assert_eq!(evicted.len(), 7);
    assert_eq!(world.tiles.tiles.len(), 3);
}

#[cfg(all(feature = "headless", feature = "thread-safe-futures"))]
#[tokio::test]
async fn tile_eviction_clears_vector_and_symbol_gpu_geometry_together() {
    use crate::{
        headless::create_headless_renderer,
        render::{eventually::Eventually, settings::BufferPoolSizes},
        sdf::SymbolBufferPool,
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
    let mut symbols = SymbolBufferPool::from_device_with_sizes(&renderer.device, sizes);
    let coords = WorldTileCoords::default();
    insert_gpu_layer(&mut vectors, &renderer.queue, coords);
    insert_gpu_layer(&mut symbols, &renderer.queue, coords);
    let mut world = World::default();
    spawn_vector(&mut world, coords, true);
    world.resources.insert(Eventually::Initialized(vectors));
    world.resources.insert(Eventually::Initialized(symbols));
    let evicted = evict_beyond(
        &mut world,
        &HashSet::new(),
        CacheBudget { tiles: 0, bytes: 0 },
    );
    assert_eq!(evicted, vec![coords]);
    super::drop_gpu_data(&mut world, &evicted);
    let Some(Eventually::Initialized(vectors)) =
        world.resources.get::<Eventually<VectorBufferPool>>()
    else {
        panic!("vector pool")
    };
    let Some(Eventually::Initialized(symbols)) =
        world.resources.get::<Eventually<SymbolBufferPool>>()
    else {
        panic!("symbol pool")
    };
    assert!(vectors.get_loaded_style_layers_at(coords).is_none());
    assert!(symbols.get_loaded_style_layers_at(coords).is_none());
}

#[cfg(all(feature = "headless", feature = "thread-safe-futures"))]
fn insert_gpu_layer<V: bytemuck::Pod, FM: bytemuck::Pod>(
    pool: &mut crate::vector::resource::BufferPool<
        wgpu::Queue,
        wgpu::Buffer,
        V,
        u32,
        crate::render::shaders::ShaderLayerMetadata,
        FM,
    >,
    queue: &wgpu::Queue,
    coords: WorldTileCoords,
) {
    let geometry = crate::vector::tessellation::OverAlignedVertexBuffer::from_iters(
        [V::zeroed(); 4],
        [0, 1, 2, 2, 1, 3],
        6,
    );
    pool.allocate_layer_geometry(
        queue,
        coords,
        crate::style::layer::StyleLayer::default(),
        &geometry,
        crate::render::shaders::ShaderLayerMetadata::new(0.0, 0.0, [0.0; 2]),
        &[FM::zeroed(); 4],
    )
    .expect("fits");
}

fn wide_eye_view() -> crate::render::view_state::ViewState {
    use cgmath::{Deg, Matrix4, Rad};

    use crate::{
        coords::{LatLon, WorldCoords, Zoom},
        projection::ProjectionType,
        render::{
            camera::EyeFrustum,
            view_state::{CameraPose, ExternalView, ViewState},
        },
        window::PhysicalSize,
    };

    let zoom = Zoom::new(14.0);
    let eye_at = LatLon::new(47.26, 11.39);
    let size = PhysicalSize::new(3840, 2160).expect("a viewport");
    let fovy = Rad(1.2);
    let mut own = ViewState::new(
        size,
        WorldCoords::from_lat_lon(eye_at, zoom),
        zoom,
        Deg(0.0),
        fovy,
    );
    own.set_max_pitch(Deg(180.0));
    own.set_camera_pose(CameraPose {
        position: eye_at,
        altitude_meters: 4000.0,
        bearing: Deg(0.0),
        pitch: Deg(60.0),
        roll: Deg(0.0),
    });
    // A wide level gaze, four kilometres up: what a headset sees from the immersive placement.
    let base = own.external_view();
    let level = ExternalView {
        view: Matrix4::from_angle_x(Deg(-30.0)) * base.view,
        frustum: EyeFrustum {
            left: 1.2,
            right: 1.2,
            top: 0.9,
            bottom: 0.9,
            near: 0.1,
            far: 1.0e9,
        },
        ..base
    };
    let mut eyed = ViewState::new(
        size,
        WorldCoords::from_lat_lon(eye_at, zoom),
        zoom,
        Deg(0.0),
        fovy,
    );
    eyed.set_external_view(level, &ProjectionType::Mercator)
        .expect("the eye is accepted");

    eyed
}

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
    io::tile_retry::{self, RequestKind},
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
            failed: false,
            done,
            layers: Vec::new(),
            overscaled_zoom: 0,
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
    for (coords, kind) in [
        (loading, RequestKind::Vector),
        (tile(700), RequestKind::Dem),
    ] {
        let attempt = tile_retry::next_attempt(&mut world);
        tile_retry::started(&mut world, coords, kind, attempt, None);
    }

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
                failed: false,
                done: true,
                overscaled_zoom: 0,
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
                        feature_sort_keys: Vec::new(),
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
        crate::render::upload_queue::UploadQueue,
        wgpu::Buffer,
        V,
        u32,
        crate::render::shaders::ShaderLayerMetadata,
        FM,
    >,
    queue: &crate::render::upload_queue::UploadQueue,
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

#[test]
fn settled_fine_tiles_stop_retaining_coarse_dem_and_raster_ancestors() {
    use rustc_hash::FxHashSet;

    use crate::{
        raster::{
            AvailableRasterLayerData, RasterLayerData, RasterLayersDataComponent, RasterSourceId,
        },
        terrain::{DemTile, LoadedDem},
    };

    let style: crate::style::Style = serde_json::from_str(
        r#"{
            "version": 8,
            "sources": {
                "ortho": { "type": "raster", "tiles": ["https://img.example/{z}/{x}/{y}.png"], "tileSize": 256, "maxzoom": 14 },
                "dem": { "type": "raster-dem", "tiles": ["https://dem.example/{z}/{x}/{y}.png"], "tileSize": 256, "maxzoom": 12, "encoding": "terrarium" }
            },
            "layers": [{ "id": "ortho", "type": "raster", "source": "ortho" }],
            "terrain": { "source": "dem", "exaggeration": 1 }
        }"#,
    )
    .expect("style parses");

    let mut world = World::default();
    let fine_view = WorldTileCoords::from((256, 256, ZoomLevel::new(10)));
    let fine_dem = WorldTileCoords::from((128, 128, ZoomLevel::new(9)));
    let coarse_raster = WorldTileCoords::from((32, 32, ZoomLevel::new(7)));
    let coarse_dem = WorldTileCoords::from((8, 8, ZoomLevel::new(5)));
    let root = WorldTileCoords::from((0, 0, ZoomLevel::new(0)));

    let ortho = RasterSourceId::from("ortho");
    let img = image::RgbaImage::new(4, 4);
    let dem = LoadedDem::new(
        DemTile::from_image(&img, [256.0, 1.0, 1.0 / 256.0, -32768.0]).expect("dem"),
    );

    for coords in [root, coarse_dem, coarse_raster] {
        world
            .tiles
            .spawn_mut(coords)
            .expect("valid")
            .insert(RasterLayersDataComponent {
                layers: vec![RasterLayerData::Available(AvailableRasterLayerData {
                    coords,
                    source: ortho.clone(),
                    image: img.clone(),
                })],
            })
            .insert(DemTileComponent::Loaded(dem.clone()));
    }

    // While the fine view tile and its z9 DEM tile have not loaded, fallback ancestors stay in use.
    let pending = super::finish_tiles_in_use(&world, &style, FxHashSet::from_iter([fine_view]));
    assert!(
        pending.contains(&coarse_raster),
        "unsettled fine tile keeps z-3 raster ancestor"
    );
    assert!(
        pending.contains(&coarse_dem),
        "unsettled DEM tile keeps z5 culling ancestor"
    );
    assert!(
        !pending.contains(&root),
        "nearest loaded fallback suffices without pinning z0 forever"
    );

    // Once the fine view tile's raster and its z9 DEM settle, coarse ancestors leave `in_use`.
    world
        .tiles
        .spawn_mut(fine_view)
        .expect("valid")
        .insert(RasterLayersDataComponent {
            layers: vec![RasterLayerData::Available(AvailableRasterLayerData {
                coords: fine_view,
                source: ortho,
                image: img,
            })],
        });
    world
        .tiles
        .spawn_mut(fine_dem)
        .expect("valid")
        .insert(DemTileComponent::Loaded(dem));

    let settled = super::finish_tiles_in_use(&world, &style, FxHashSet::from_iter([fine_view]));
    assert!(settled.contains(&fine_view));
    assert!(settled.contains(&fine_dem));
    assert!(
        !settled.contains(&coarse_raster),
        "settled fine raster drops coarse ancestor"
    );
    assert!(
        !settled.contains(&coarse_dem),
        "settled fine DEM drops z5 ancestor"
    );
    assert!(
        !settled.contains(&root),
        "settled fine tile drops z0 ancestor"
    );
}

#[test]
fn wide_view_tile_cache_is_capped_at_max_cache_tiles() {
    let mut world = World::default();
    for index in 0..(super::MAX_CACHE_TILES + 40) as i32 {
        let coords = WorldTileCoords {
            x: index % 64,
            y: index / 64,
            z: ZoomLevel::new(7),
        };
        spawn_vector(&mut world, coords, true);
    }
    let evicted = evict_stale_tiles(&mut world, &HashSet::new(), 200);
    assert_eq!(evicted.len(), 40);
    assert_eq!(world.tiles.tiles.len(), super::MAX_CACHE_TILES);
}

#![expect(
    clippy::expect_used,
    reason = "benchmark setup and query runs abort on invalid test fixtures"
)]

use std::hint::black_box;

use cgmath::{Deg, Vector2};
use criterion::{
    criterion_group, criterion_main, measurement::WallTime, BenchmarkGroup, Criterion,
};
use image::{Rgba, RgbaImage};
use maplibre::{
    coords::{WorldCoords, WorldTileCoords, Zoom, ZoomLevel, TILE_SIZE},
    io::geometry_index::wire::{WireGeometry, WireTileIndex},
    query::query_rendered_features,
    render::{
        projection::{globe_camera_for_view, projection_data_for_view},
        resolved_view::ResolvedFrameView,
        view_state::ViewState,
    },
    sdf::query::{QueryGeometry, QueryOptions},
    style::Style,
    tcs::world::World,
    terrain::{DemTile, DemTileComponent, LoadedDem},
    window::PhysicalSize,
};

fn encode_height_prop(height: f64) -> Vec<u8> {
    let mut out = Vec::with_capacity(24);
    out.extend_from_slice(&1_u32.to_le_bytes());
    let key = b"height";
    out.extend_from_slice(&(key.len() as u32).to_le_bytes());
    out.extend_from_slice(key);
    out.push(3_u8); // TAG_NUMBER
    out.extend_from_slice(&height.to_le_bytes());
    out
}

fn add_prop_group(wire: &mut WireTileIndex, height: f64) -> u32 {
    let idx = (wire.prop_offsets.len() - 1) as u32;
    wire.prop_blobs
        .extend_from_slice(&encode_height_prop(height));
    wire.prop_offsets.push(wire.prop_blobs.len() as u32);
    idx
}

fn append_city_buildings_and_landuse(wire: &mut WireTileIndex) {
    // 200 extruded buildings + 100 landuse polygons across the 4096-unit tile.
    for row in 0..10_u64 {
        for col in 0..20_u64 {
            let cx = 120.0 + col as f64 * 195.0;
            let cy = 200.0 + row as f64 * 370.0;
            let half = 55.0;
            let id = row * 20 + col + 1;
            let prop_idx = add_prop_group(wire, 24.0 + (id % 40) as f64);

            for (layer_idx, feat_id) in [(0_u32, id), (1_u32, 1000 + id)] {
                if layer_idx == 1 && id % 2 != 0 {
                    continue;
                }
                let coord_start = (wire.xy.len() / 2) as u32;
                let ring_start = wire.ring_ends.len() as u32;
                for (px, py) in [
                    (cx - half, cy - half),
                    (cx + half, cy - half),
                    (cx + half, cy + half),
                    (cx - half, cy + half),
                    (cx - half, cy - half),
                ] {
                    wire.xy.push(px);
                    wire.xy.push(py);
                }
                wire.ring_ends.push(5);
                wire.geometries.push(WireGeometry {
                    id: feat_id,
                    source_layer_idx: layer_idx,
                    prop_group_idx: prop_idx,
                    feature_index: feat_id as u32,
                    ring_start,
                    ring_count: 1,
                    coord_start,
                    coord_count: 5,
                    kind: 2,
                    has_id: true,
                });
            }
        }
    }
}

fn append_city_roads_and_pois(wire: &mut WireTileIndex, empty_prop_idx: u32) {
    // 50 roads + 100 POIs.
    for i in 0..50_u64 {
        let y = 80.0 + i as f64 * 80.0;
        let coord_start = (wire.xy.len() / 2) as u32;
        for (px, py) in [(0.0, y), (2048.0, y), (4096.0, y)] {
            wire.xy.push(px);
            wire.xy.push(py);
        }
        wire.geometries.push(WireGeometry {
            id: 2000 + i,
            source_layer_idx: 2,
            prop_group_idx: empty_prop_idx,
            feature_index: (2000 + i) as u32,
            ring_start: 0,
            ring_count: 0,
            coord_start,
            coord_count: 3,
            kind: 1,
            has_id: true,
        });
    }
    for i in 0..100_u64 {
        let x = 100.0 + (i % 10) as f64 * 400.0;
        let y = 100.0 + (i / 10) as f64 * 400.0;
        let coord_start = (wire.xy.len() / 2) as u32;
        wire.xy.push(x);
        wire.xy.push(y);
        wire.geometries.push(WireGeometry {
            id: 3000 + i,
            source_layer_idx: 3,
            prop_group_idx: empty_prop_idx,
            feature_index: (3000 + i) as u32,
            ring_start: 0,
            ring_count: 0,
            coord_start,
            coord_count: 1,
            kind: 0,
            has_id: true,
        });
    }
}

fn populate_city_tile(world: &mut World, coords: WorldTileCoords) {
    let mut wire = WireTileIndex {
        is_spatial: true,
        strings: vec![
            "building".into(),
            "landuse".into(),
            "road".into(),
            "poi".into(),
        ],
        prop_blobs: Vec::new(),
        prop_offsets: vec![0],
        geometries: Vec::with_capacity(450),
        ring_ends: Vec::with_capacity(300),
        xy: Vec::with_capacity(4000),
    };
    append_city_buildings_and_landuse(&mut wire);
    let empty_prop_idx = add_prop_group(&mut wire, 0.0);
    append_city_roads_and_pois(&mut wire, empty_prop_idx);

    let _spawned = world.tiles.spawn_mut(coords);
    world
        .tiles
        .geometry_index
        .index_tile(&coords, Some("city".to_owned()), wire.into_tile_index());
}

fn city_style(projection: Option<&str>, terrain: bool) -> Style {
    let mut doc = serde_json::json!({
        "version": 8,
        "sources": {
            "city": {"type": "vector"},
            "dem": {
                "type": "raster-dem",
                "tiles": ["https://example.invalid/{z}/{x}/{y}.png"],
                "encoding": "terrarium",
                "tileSize": 64,
                "maxzoom": 14
            }
        },
        "layers": [
            {"id": "landuse", "type": "fill", "source": "city", "source-layer": "landuse"},
            {"id": "road", "type": "line", "source": "city", "source-layer": "road", "paint": {"line-width": 4}},
            {"id": "poi", "type": "circle", "source": "city", "source-layer": "poi", "paint": {"circle-radius": 6}},
            {"id": "building", "type": "fill-extrusion", "source": "city", "source-layer": "building", "paint": {"fill-extrusion-height": ["get", "height"]}}
        ]
    });
    if let Some(obj) = doc.as_object_mut() {
        if let Some(proj) = projection {
            obj.insert("projection".to_owned(), serde_json::json!({"type": proj}));
        }
        if terrain {
            obj.insert(
                "terrain".to_owned(),
                serde_json::json!({"source": "dem", "exaggeration": 1.0}),
            );
        }
    }
    serde_json::from_value(doc).expect("valid benchmark fixture")
}

fn bench_view_snapshot(c: &mut Criterion) {
    let style = city_style(Some("globe"), false);
    let zoom = Zoom::new(11.5);
    let ws = TILE_SIZE * 2.0_f64.powf(zoom.value());
    let view = ViewState::new(
        PhysicalSize::new(1280, 800).expect("valid benchmark fixture"),
        WorldCoords::at_ground(ws * 0.52, ws * 0.36),
        zoom,
        Deg(40.0),
        Deg(36.87),
    );

    let mut group = c.benchmark_group("frame_view_resolution");
    // Legacy per-frame cost across 3 consumers (render upload + symbol/covering + feature query),
    // each independently deriving matrices and GlobeCameraState from ViewState + Style.
    group.bench_function("legacy_3_consumers_recompute", |b| {
        b.iter(|| {
            let s = black_box(&style);
            let v = black_box(&view);
            let gpu_vp = v.gpu_view_projection();
            let shader = projection_data_for_view(s, v).expect("valid benchmark fixture");
            let vp = v.view_projection();
            let inv = v
                .inverted_view_projection()
                .expect("valid benchmark fixture");
            let globe_a = globe_camera_for_view(v).expect("valid benchmark fixture");
            let inv_q = v
                .inverted_view_projection()
                .expect("valid benchmark fixture");
            for corner in [[640.0, 400.0]; 4] {
                black_box(v.window_to_world_at_ground(
                    &Vector2::new(corner[0], corner[1]),
                    &inv_q,
                    true,
                ));
            }
            let globe_b = globe_camera_for_view(v).expect("valid benchmark fixture");
            black_box((gpu_vp, shader, vp, inv, globe_a, globe_b));
        });
    });

    // New architecture: ResolvedFrameView::resolve runs once per frame in resolve_frame_view_system,
    // and downstream consumers borrow the immutable snapshot in O(1) via get_or_resolve.
    group.bench_function("resolved_frame_view_1_resolve_plus_2_cached", |b| {
        let mut world = World::default();
        b.iter(|| {
            let s = black_box(&style);
            let v = black_box(&view);
            let snapshot = ResolvedFrameView::resolve(s, v).expect("valid benchmark fixture");
            world.resources.insert(snapshot);
            let c1 =
                ResolvedFrameView::get_or_resolve(&world, s, v).expect("valid benchmark fixture");
            let c2 =
                ResolvedFrameView::get_or_resolve(&world, s, v).expect("valid benchmark fixture");
            black_box((c1.shader_projection, c2.gpu_view_projection.downcast()));
        });
    });
    group.finish();
}

fn bench_flat_mercator_query(group: &mut BenchmarkGroup<'_, WallTime>) {
    let style = city_style(None, false);
    let zoom = Zoom::new(14.0);
    let tile = WorldTileCoords {
        x: 8520,
        y: 5740,
        z: ZoomLevel::new(14),
    };
    let tile_px = TILE_SIZE / zoom.scale_to_zoom_level(tile.z);
    let center = [
        (tile.x as f64 + 0.5) * tile_px,
        (tile.y as f64 + 0.5) * tile_px,
    ];
    let view = ViewState::new(
        PhysicalSize::new(1280, 800).expect("valid benchmark fixture"),
        WorldCoords::at_ground(center[0], center[1]),
        zoom,
        Deg(45.0),
        Deg(36.87),
    );
    let mut world = World::default();
    populate_city_tile(&mut world, tile);
    world
        .resources
        .insert(ResolvedFrameView::resolve(&style, &view).expect("valid benchmark fixture"));

    group.bench_function("flat_mercator_point_450_geoms", |b| {
        b.iter(|| {
            let hits = query_rendered_features(
                black_box(&world),
                black_box(&style),
                black_box(&view),
                black_box(QueryGeometry::Point([640.0, 400.0])),
                &QueryOptions::default(),
            )
            .expect("valid benchmark fixture");
            black_box(hits);
        });
    });
}

fn bench_globe_3d_query(group: &mut BenchmarkGroup<'_, WallTime>) {
    let style = city_style(Some("globe"), false);
    let zoom = Zoom::new(2.0);
    let ws = TILE_SIZE * 2.0_f64.powf(zoom.value());
    let tile = WorldTileCoords {
        x: 2,
        y: 2,
        z: ZoomLevel::new(2),
    };
    let view = ViewState::new(
        PhysicalSize::new(1280, 800).expect("valid benchmark fixture"),
        WorldCoords::at_ground(ws * 0.55, ws * 0.55),
        zoom,
        Deg(20.0),
        Deg(36.87),
    );
    let mut world = World::default();
    populate_city_tile(&mut world, tile);
    world
        .resources
        .insert(ResolvedFrameView::resolve(&style, &view).expect("valid benchmark fixture"));

    group.bench_function("globe_3d_point_450_geoms", |b| {
        b.iter(|| {
            let hits = query_rendered_features(
                black_box(&world),
                black_box(&style),
                black_box(&view),
                black_box(QueryGeometry::Point([640.0, 400.0])),
                &QueryOptions::default(),
            )
            .expect("valid benchmark fixture");
            black_box(hits);
        });
    });
}

fn bench_terrain_3d_query(group: &mut BenchmarkGroup<'_, WallTime>) {
    let style = city_style(None, true);
    let zoom = Zoom::new(12.0);
    let tile = WorldTileCoords {
        x: 2130,
        y: 1435,
        z: ZoomLevel::new(12),
    };
    let tile_px = TILE_SIZE / zoom.scale_to_zoom_level(tile.z);
    let center = [
        (tile.x as f64 + 0.5) * tile_px,
        (tile.y as f64 + 0.5) * tile_px,
    ];
    let mut view = ViewState::new(
        PhysicalSize::new(1280, 800).expect("valid benchmark fixture"),
        WorldCoords::at_ground(center[0], center[1]),
        zoom,
        Deg(55.0),
        Deg(36.87),
    );
    view.set_center_elevation(1200.0);

    let unpack = [256.0, 1.0, 1.0 / 256.0, 32768.0];
    let encoded = 1200.0_f64 + 32768.0;
    let r = (encoded / 256.0).floor() as u8;
    let g = (encoded - f64::from(r) * 256.0).floor() as u8;
    let dem_tile =
        DemTile::from_image(&RgbaImage::from_pixel(64, 64, Rgba([r, g, 0, 255])), unpack)
            .expect("valid benchmark fixture");

    let mut world = World::default();
    for dy in -2..=2 {
        for dx in -2..=2 {
            let c = WorldTileCoords {
                x: tile.x + dx,
                y: tile.y + dy,
                z: tile.z,
            };
            let _spawned = world.tiles.spawn_mut(c);
            if let Some(dem_c) = c.get_parent() {
                world
                    .tiles
                    .spawn_mut(dem_c)
                    .expect("valid benchmark fixture")
                    .insert(DemTileComponent::Loaded(LoadedDem::new(dem_tile.clone())));
            }
        }
    }
    populate_city_tile(&mut world, tile);
    let resolved = ResolvedFrameView::resolve(&style, &view).expect("valid benchmark fixture");
    let ti = maplibre::terrain::coverage::TerrainCoverageIndex::build(
        world.tiles.coords(),
        &world.tiles,
        &maplibre::terrain::source::dem_source(&style).expect("valid benchmark fixture"),
    );
    world.resources.insert(resolved);
    world.resources.insert(ti);

    group.bench_function("terrain_3d_pitched_point_450_geoms", |b| {
        b.iter(|| {
            let hits = query_rendered_features(
                black_box(&world),
                black_box(&style),
                black_box(&view),
                black_box(QueryGeometry::Point([640.0, 380.0])),
                &QueryOptions::default(),
            )
            .expect("valid benchmark fixture");
            black_box(hits);
        });
    });
}

fn bench_wire_roundtrip_query(group: &mut BenchmarkGroup<'_, WallTime>) {
    let mut world = World::default();
    let tile = WorldTileCoords {
        x: 8520,
        y: 5740,
        z: ZoomLevel::new(14),
    };
    populate_city_tile(&mut world, tile);
    let (_, src_index) = world
        .tiles
        .geometry_index
        .tile_indexes(&tile)
        .expect("valid benchmark fixture")
        .next()
        .expect("valid benchmark fixture");
    let spatial = WireTileIndex::from_tile_index(src_index).into_tile_index();

    group.bench_function("worker_wire_tile_index_roundtrip_450_geoms", |b| {
        b.iter(|| {
            let wire = WireTileIndex::from_tile_index(black_box(&spatial));
            let decoded = wire.into_tile_index();
            black_box(decoded);
        });
    });
}

fn bench_cross_projection_queries(c: &mut Criterion) {
    let mut group = c.benchmark_group("query_rendered_features");
    bench_flat_mercator_query(&mut group);
    bench_globe_3d_query(&mut group);
    bench_terrain_3d_query(&mut group);
    bench_wire_roundtrip_query(&mut group);
    group.finish();
}

criterion_group!(
    name = benches;
    config = Criterion::default().significance_level(0.05).sample_size(40);
    targets = bench_view_snapshot, bench_cross_projection_queries
);
criterion_main!(benches);

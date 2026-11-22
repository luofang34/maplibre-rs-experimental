//! Live GPU resource lifecycle verification tests (C3-G1, C3-G2, C3-G3):
//! 1. 20x Flat <-> Globe projection toggle: bounded GPU objects & pixel parity on second round-trip.
//! 2. Symbol layer/style removal: shared sampler + dropped SymbolTextures/bind groups/atlases.
//! 3. Terrain disable: resets `Eventually<TerrainResources>` & releases unused `GpuDemPool` entries.
//! 4. Rapid pan/zoom tile eviction: GPU buffers/textures plateau within memory budget.
//! 5. Repeated `set_style` layer/source churn: zero monotonic GPU object leaks.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::expect_used, clippy::panic)]

#[path = "gpu_lifecycle/helpers.rs"]
mod helpers;

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use helpers::{
    capture_pixels, gpu_counts, make_map, max_channel_diff, minimal_mvt, projection_style,
    settle_frames, solid_rgb_png, MockServer,
};
use maplibre::{
    coords::{LatLon, WorldTileCoords},
    headless::map::{process_geojson_layers, HeadlessMap},
    projection::ProjectionType,
    render::{eventually::Eventually, view_state::CameraOptions},
    sdf::PlacementConfig,
    style::Style,
    terrain::{gpu_dem_pool::GpuDemPool, resources::TerrainResources},
};

// 1. 20x Flat <-> Globe toggle keeps GPU objects bounded and preserves second round-trip pixel parity.
#[test]
fn flat_globe_toggle_20x_keeps_gpu_resources_bounded_and_pixel_parity() {
    futures::executor::block_on(async {
        let flat = projection_style(false);
        let globe = projection_style(true);
        let (mut map, mut readback) = make_map(flat.clone(), None).await;

        let match_layers = map.context().style.layers[1..2].to_vec();
        let gj = serde_json::json!({
            "type": "FeatureCollection",
            "features": [{
                "type": "Feature",
                "geometry": {
                    "type": "Polygon",
                    "coordinates": [[[-30.0, -30.0], [30.0, -30.0], [30.0, 30.0], [-30.0, 30.0], [-30.0, -30.0]]]
                },
                "properties": {}
            }]
        });
        let tile = process_geojson_layers(
            &gj,
            "gj",
            match_layers,
            WorldTileCoords::default(),
            ProjectionType::Mercator,
        )
        .expect("gj");
        map.render_tile(tile).expect("tile");
        settle_frames(&mut map, 3);
        let baseline_flat = capture_pixels(&map, &mut readback);

        map.set_style(globe.clone());
        settle_frames(&mut map, 3);
        let baseline_globe = capture_pixels(&map, &mut readback);

        map.set_style(flat.clone());
        settle_frames(&mut map, 3);
        let after_warm_counts = gpu_counts(&mut map);

        for i in 0..20 {
            map.set_style(if i % 2 == 0 {
                globe.clone()
            } else {
                flat.clone()
            });
            settle_frames(&mut map, 2);
        }

        map.set_style(globe);
        settle_frames(&mut map, 3);
        let final_globe = capture_pixels(&map, &mut readback);

        map.set_style(flat);
        settle_frames(&mut map, 3);
        let final_flat = capture_pixels(&map, &mut readback);
        let end_counts = gpu_counts(&mut map);

        assert!(
            max_channel_diff(&baseline_flat, &final_flat) <= 2,
            "flat pixels drifted after 20x toggle"
        );
        assert!(
            max_channel_diff(&baseline_globe, &final_globe) <= 2,
            "globe pixels drifted after 20x toggle"
        );
        assert!(
            end_counts.buffers <= after_warm_counts.buffers + 2
                && end_counts.textures <= after_warm_counts.textures + 2
                && end_counts.bind_groups <= after_warm_counts.bind_groups + 4
                && end_counts.pipelines <= after_warm_counts.pipelines + 2,
            "GPU resource leak across 20x toggle: warm={after_warm_counts:?}, end={end_counts:?}"
        );
    });
}

// 2. Symbol style -> empty style drops SymbolTextures, atlases, bind groups & shared sampler (C3-G2).
#[test]
fn symbol_style_to_empty_style_releases_symbol_textures_and_shares_one_sampler() {
    futures::executor::block_on(async {
        let sym_style: Style = serde_json::from_value(serde_json::json!({
            "version": 8,
            "sources": {"pts": {"type": "geojson", "data": {"type": "FeatureCollection", "features": []}}},
            "layers": [
                {"id": "bg", "type": "background", "paint": {"background-color": "#ffffff"}},
                {"id": "sym-a", "type": "symbol", "source": "pts", "layout": {"text-field": "{t}", "text-size": 20}},
                {"id": "sym-b", "type": "symbol", "source": "pts", "layout": {"text-field": "{t}", "text-size": 24}}
            ]
        }))
        .expect("sym");
        let empty_style: Style = serde_json::from_value(serde_json::json!({
            "version": 8,
            "sources": {},
            "layers": [{"id": "bg", "type": "background", "paint": {"background-color": "#ffffff"}}]
        }))
        .expect("empty");

        let (mut map, _) = make_map(empty_style.clone(), None).await;
        settle_frames(&mut map, 2);
        let empty_counts = gpu_counts(&mut map);

        let upload_symbols = |m: &mut HeadlessMap, s: &Style| {
            m.set_style(s.clone());
            m.set_placement_config(PlacementConfig::immediate());
            let layers = m.context().style.layers[1..].to_vec();
            let gj = serde_json::json!({
                "type": "FeatureCollection",
                "features": [{
                    "type": "Feature",
                    "geometry": {"type": "Point", "coordinates": [0.0, 0.0]},
                    "properties": {"t": "AB"}
                }]
            });
            let tile = process_geojson_layers(
                &gj,
                "pts",
                layers,
                WorldTileCoords::default(),
                ProjectionType::Mercator,
            )
            .expect("gj");
            m.render_tile(tile).expect("tile");
            settle_frames(m, 3);
        };

        upload_symbols(&mut map, &sym_style);
        let with_sym = gpu_counts(&mut map);
        // Shared sampler across both symbol layers means sampler count increases by at most 1 over empty.
        assert!(
            with_sym.samplers <= empty_counts.samplers + 1,
            "expected shared sampler for SymbolTextures: empty={empty_counts:?}, sym={with_sym:?}"
        );

        map.set_style(empty_style.clone());
        settle_frames(&mut map, 3);
        let after_clear = gpu_counts(&mut map);
        assert!(
            after_clear.textures <= empty_counts.textures + 1
                && after_clear.samplers <= empty_counts.samplers,
            "SymbolTextures were not released on empty style: empty={empty_counts:?}, after={after_clear:?}"
        );

        // Repeat once to verify steady-state idempotency.
        upload_symbols(&mut map, &sym_style);
        map.set_style(empty_style);
        settle_frames(&mut map, 3);
        let after_second_clear = gpu_counts(&mut map);
        assert_eq!(after_second_clear.samplers, after_clear.samplers);
    });
}

// 3. Terrain disable resets Eventually<TerrainResources> & drops GpuDemPool entries (C3-G3).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terrain_disable_resets_terrain_resources_and_drops_unused_dem_textures() {
    let dem_png = solid_rgb_png(256, 256, 128, 0, 0);
    let server = MockServer::new(move |_| dem_png.clone());

    let no_terrain: Style = serde_json::from_value(serde_json::json!({
        "version": 8,
        "center": [0.0, 0.0],
        "zoom": 1.0,
        "sources": {},
        "layers": [{"id": "bg", "type": "background", "paint": {"background-color": "#204060"}}]
    }))
    .expect("base");
    let with_terrain: Style = serde_json::from_value(serde_json::json!({
        "version": 8,
        "center": [0.0, 0.0],
        "zoom": 1.0,
        "sources": {
            "dem": {"type": "raster-dem", "tiles": ["https://example.invalid/{z}/{x}/{y}.png"], "tileSize": 256, "encoding": "terrarium"}
        },
        "terrain": {"source": "dem", "exaggeration": 1.0},
        "layers": [{"id": "bg", "type": "background", "paint": {"background-color": "#204060"}}]
    }))
    .expect("terrain");

    let (mut map, _) = make_map(no_terrain.clone(), Some(server)).await;
    settle_frames(&mut map, 2);
    let flat_counts = gpu_counts(&mut map);

    map.set_style(with_terrain.clone());
    for _ in 0..80 {
        map.frame_input_mut().advance(Duration::from_millis(16));
        map.run_frame().expect("frame");
        let _ = map.device().poll(wgpu::PollType::wait_indefinitely());
        tokio::time::sleep(Duration::from_millis(10)).await;
        let loaded = map
            .context()
            .world
            .resources
            .get::<GpuDemPool>()
            .is_some_and(|p| p.texture_count() > 0)
            && matches!(
                map.context()
                    .world
                    .resources
                    .get::<Eventually<TerrainResources>>(),
                Some(Eventually::Initialized(_))
            );
        if loaded {
            break;
        }
    }
    assert!(
        matches!(
            map.context()
                .world
                .resources
                .get::<Eventually<TerrainResources>>(),
            Some(Eventually::Initialized(_))
        ),
        "expected TerrainResources initialized"
    );
    assert!(
        map.context()
            .world
            .resources
            .get::<GpuDemPool>()
            .is_some_and(|p| p.texture_count() > 0),
        "expected GpuDemPool populated"
    );

    map.set_style(no_terrain);
    settle_frames(&mut map, 3);
    let after_disable = gpu_counts(&mut map);

    assert!(
        !matches!(
            map.context()
                .world
                .resources
                .get::<Eventually<TerrainResources>>(),
            Some(Eventually::Initialized(_))
        ),
        "TerrainResources must reset to Uninitialized when terrain is disabled"
    );
    assert_eq!(
        map.context()
            .world
            .resources
            .get::<GpuDemPool>()
            .map_or(0, |p| p.texture_count()),
        0,
        "unused DEM source textures must drop from GpuDemPool"
    );
    assert!(
        after_disable.textures <= flat_counts.textures + 2
            && after_disable.bind_groups <= flat_counts.bind_groups + 2,
        "terrain GPU resources leaked after disable: flat={flat_counts:?}, after={after_disable:?}"
    );
}

// 4. Rapid pan/zoom tile eviction keeps GPU resource counts bounded.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rapid_pan_and_zoom_evicts_tile_gpu_resources_within_budget() {
    let mvt = minimal_mvt("water");
    let counts_map = Arc::new(Mutex::new(HashMap::<String, usize>::new()));
    let c2 = counts_map.clone();
    let server = MockServer::new(move |url| {
        *c2.lock().expect("lock").entry(url.to_owned()).or_insert(0) += 1;
        mvt.clone()
    });

    let style: Style = serde_json::from_value(serde_json::json!({
        "version": 8,
        "center": [0.0, 0.0],
        "zoom": 4.0,
        "sources": {"vec": {"type": "vector", "tiles": ["https://example.invalid/{z}/{x}/{y}.mvt"], "maxzoom": 8}},
        "layers": [{"id": "water", "type": "fill", "source": "vec", "source-layer": "water", "paint": {"fill-color": "#3366aa"}}]
    }))
    .expect("style");
    let (mut map, _) = make_map(style, Some(server)).await;
    map.set_available_memory(Some(8 * 1024 * 1024));

    for step in 0..8 {
        let _ = map.jump_to(CameraOptions {
            center: Some(LatLon::new(
                -20.0 + step as f64 * 5.0,
                -40.0 + step as f64 * 10.0,
            )),
            zoom: Some(4.0 + (step % 3) as f64),
            ..Default::default()
        });
        settle_frames(&mut map, 2);
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let warm = gpu_counts(&mut map);

    for step in 8..32 {
        let _ = map.jump_to(CameraOptions {
            center: Some(LatLon::new(
                -50.0 + (step % 15) as f64 * 6.0,
                -160.0 + step as f64 * 11.0,
            )),
            zoom: Some(5.0 + (step % 3) as f64),
            ..Default::default()
        });
        settle_frames(&mut map, 2);
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    settle_frames(&mut map, 4);
    let end = gpu_counts(&mut map);

    assert!(
        end.buffers <= warm.buffers + 24 && end.textures <= warm.textures + 12,
        "GPU objects climbed across pan/zoom eviction: warm={warm:?}, end={end:?}"
    );
}

// 5. Repeated set_style layer/source churn leaves no stale GPU objects.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_style_layer_and_source_churn_leaves_no_stale_gpu_resources() {
    let png = solid_rgb_png(64, 64, 40, 120, 200);
    let server = MockServer::new(move |_| png.clone());

    let minimal: Style = serde_json::from_value(serde_json::json!({
        "version": 8,
        "sources": {},
        "layers": [{"id": "bg", "type": "background", "paint": {"background-color": "#000000"}}]
    }))
    .expect("min");
    let (mut map, _) = make_map(minimal.clone(), Some(server)).await;

    let run_cycle = |m: &mut HeadlessMap, cycle: usize| {
        let rich: Style = serde_json::from_value(serde_json::json!({
            "version": 8,
            "center": [cycle as f64 * 2.0, 0.0],
            "zoom": 1.0,
            "sources": {
                format!("r-{cycle}"): {"type": "raster", "tiles": [format!("https://example.invalid/{cycle}/{{z}}/{{x}}/{{y}}.png")], "tileSize": 256}
            },
            "layers": [
                {"id": "bg", "type": "background", "paint": {"background-color": "#112233"}},
                {"id": format!("rl-{cycle}"), "type": "raster", "source": format!("r-{cycle}")}
            ]
        }))
        .expect("rich");
        m.set_style(rich);
        settle_frames(m, 3);
    };

    // Warm up one-time static raster pipeline/samplers, then capture clean baseline.
    run_cycle(&mut map, 0);
    tokio::time::sleep(Duration::from_millis(15)).await;
    settle_frames(&mut map, 2);
    map.set_style(minimal.clone());
    settle_frames(&mut map, 4);
    let base = gpu_counts(&mut map);

    for cycle in 1..9 {
        run_cycle(&mut map, cycle);
        tokio::time::sleep(Duration::from_millis(10)).await;
        settle_frames(&mut map, 2);
    }

    map.set_style(minimal);
    settle_frames(&mut map, 4);
    let end = gpu_counts(&mut map);
    assert!(
        end.buffers <= base.buffers + 2
            && end.textures <= base.textures + 2
            && end.bind_groups <= base.bind_groups + 4
            && end.samplers <= base.samplers + 1
            && end.pipelines <= base.pipelines + 1,
        "GPU resource leak after style churn: base={base:?}, end={end:?}"
    );
}

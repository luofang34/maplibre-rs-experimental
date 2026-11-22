//! Integration tests verifying empty/sparse tile semantics (C6):
//! - Headless MVT empty / missing source-layer tiles suppress parent fallback.
//! - Real HTTP loader vector 204 & 404 settle empty (suppressing parent fallback), while 500
//!   retains parent fallback and recovers on retry without panicking.
//! - Real HTTP loader raster 404 triggers parent fallback, whereas 204 suppresses parent fallback
//!   with zero GPU texture allocation.
//! - Mixed z=1 quadrants (valid, 204 empty, 404 fallback, valid) render distinct quadrant colors.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

#[path = "empty_tile/helpers.rs"]
mod helpers;

use std::time::Duration;

use helpers::{
    encode_solid_rgba_png, full_tile_mvt, make_loader_map, pixel_at, pixel_with_readback,
    pump_until, raster_style, vector_style, z1_quadrants, MockResponse, MockTileClient, SIZE,
};
use maplibre::{
    background::BackgroundPlugin,
    coords::WorldTileCoords,
    headless::{
        create_headless_renderer,
        map::{process_tile_layers, readback::FrameReadback, HeadlessMap, ProcessedLayers},
        HeadlessPlugin,
    },
    plugin::Plugin,
    projection::ProjectionType,
    raster::{resource::RasterResources, RasterLayerData, RasterLayersDataComponent},
    render::{eventually::Eventually, view_state::CameraOptions, RenderPlugin},
    vector::{DefaultVectorTransferables, VectorLayerBucketComponent, VectorPlugin},
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn empty_mvt_tile_suppresses_parent_fill_fallback() {
    let (kernel, renderer) = create_headless_renderer(SIZE, SIZE, None)
        .await
        .expect("renderer");
    let style = vector_style(1, "local://vec/{z}/{x}/{y}.mvt");
    let fill_layer = style.layers[1].clone();
    let plugins: Vec<Box<dyn Plugin<_>>> = vec![
        Box::new(RenderPlugin),
        Box::new(BackgroundPlugin),
        Box::new(VectorPlugin::<DefaultVectorTransferables>::default()),
        Box::new(HeadlessPlugin::new(false).preserve_tile_sources()),
    ];
    let mut map = HeadlessMap::new(style, renderer, kernel, plugins).expect("HeadlessMap");

    let parent_tile = full_tile_mvt("water");
    let parent_layers = process_tile_layers(
        &parent_tile,
        &fill_layer,
        WorldTileCoords::from((0, 0, 0_u8.into())),
        ProjectionType::Mercator,
    )
    .expect("parent mvt");
    map.render_tile(parent_layers).expect("render parent");
    let parent_px = pixel_at(&map, 64, 64);
    assert!(
        parent_px[0] > 200 && parent_px[2] < 50,
        "expected red parent fallback, got {parent_px:?}"
    );

    let mut empty_children = ProcessedLayers::default();
    for coords in z1_quadrants() {
        let mut processed = process_tile_layers(b"", &fill_layer, coords, ProjectionType::Mercator)
            .expect("empty mvt");
        empty_children.append(&mut processed);
    }
    map.render_tile(empty_children)
        .expect("render empty children");
    let child_px = pixel_at(&map, 64, 64);
    assert!(
        child_px[2] > 200 && child_px[0] < 50,
        "empty MVT should show blue background, got {child_px:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mvt_tile_missing_source_layer_suppresses_parent_fill_fallback() {
    let (kernel, renderer) = create_headless_renderer(SIZE, SIZE, None)
        .await
        .expect("renderer");
    let style = vector_style(1, "local://vec/{z}/{x}/{y}.mvt");
    let fill_layer = style.layers[1].clone();
    let plugins: Vec<Box<dyn Plugin<_>>> = vec![
        Box::new(RenderPlugin),
        Box::new(BackgroundPlugin),
        Box::new(VectorPlugin::<DefaultVectorTransferables>::default()),
        Box::new(HeadlessPlugin::new(false).preserve_tile_sources()),
    ];
    let mut map = HeadlessMap::new(style, renderer, kernel, plugins).expect("HeadlessMap");

    let parent_layers = process_tile_layers(
        &full_tile_mvt("water"),
        &fill_layer,
        WorldTileCoords::from((0, 0, 0_u8.into())),
        ProjectionType::Mercator,
    )
    .expect("parent");
    map.render_tile(parent_layers).expect("render parent");
    assert!(pixel_at(&map, 64, 64)[0] > 200);

    let unrelated = full_tile_mvt("roads");
    let mut children = ProcessedLayers::default();
    for coords in z1_quadrants() {
        let mut processed =
            process_tile_layers(&unrelated, &fill_layer, coords, ProjectionType::Mercator)
                .expect("other layer");
        children.append(&mut processed);
    }
    map.render_tile(children).expect("render children");
    let px = pixel_at(&map, 64, 64);
    assert!(
        px[2] > 200 && px[0] < 50,
        "missing source-layer must suppress parent fallback, got {px:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn vector_loader_204_404_and_500_parent_fallback_semantics() {
    let parent_mvt = full_tile_mvt("water");

    for status in [204_u16, 404_u16] {
        let parent_clone = parent_mvt.clone();
        let client = MockTileClient::new(move |url| {
            if url.ends_with("/0/0/0.mvt") {
                MockResponse::Bytes(parent_clone.clone())
            } else {
                MockResponse::Status(status)
            }
        });
        let mut style = vector_style(1, "http://mock.invalid/{z}/{x}/{y}.mvt");
        style.zoom = Some(0.0);
        let mut map = make_loader_map(style, client.clone(), false).await;
        pump_until(&mut map, |m| {
            m.context()
                .world
                .tiles
                .query::<&VectorLayerBucketComponent>(WorldTileCoords::from((0, 0, 0_u8.into())))
                .is_some_and(|b| b.done && !b.failed)
        })
        .await;
        assert!(pixel_at(&map, 64, 64)[0] > 200, "z0 should draw red");

        map.jump_to(CameraOptions {
            zoom: Some(1.0),
            ..Default::default()
        })
        .expect("jump_to z1");
        pump_until(&mut map, |m| {
            z1_quadrants().iter().all(|c| {
                m.context()
                    .world
                    .tiles
                    .query::<&VectorLayerBucketComponent>(*c)
                    .is_some_and(|b| b.done && !b.failed)
            })
        })
        .await;
        let px = pixel_at(&map, 64, 64);
        assert!(
            px[2] > 200 && px[0] < 50,
            "vector HTTP {status} should suppress parent fallback and show background, got {px:?}"
        );
        let initial_reqs = client.request_count("/1/");
        map.frame_input_mut().advance(Duration::from_millis(2000));
        map.run_frame().expect("steady frame");
        assert_eq!(
            client.request_count("/1/"),
            initial_reqs,
            "terminal HTTP {status} must not retry"
        );
    }

    // HTTP 500 retains z0 parent fallback and recovers cleanly when server heals on retry.
    let parent_clone = parent_mvt.clone();
    let client = MockTileClient::new(move |url| {
        if url.ends_with("/0/0/0.mvt") {
            MockResponse::Bytes(parent_clone.clone())
        } else {
            MockResponse::Status(500)
        }
    });
    let mut style = vector_style(1, "http://mock.invalid/{z}/{x}/{y}.mvt");
    style.zoom = Some(0.0);
    let mut map = make_loader_map(style, client.clone(), false).await;
    pump_until(&mut map, |m| {
        m.context()
            .world
            .tiles
            .query::<&VectorLayerBucketComponent>(WorldTileCoords::from((0, 0, 0_u8.into())))
            .is_some_and(|b| b.done && !b.failed)
    })
    .await;

    map.jump_to(CameraOptions {
        zoom: Some(1.0),
        ..Default::default()
    })
    .expect("jump_to z1");
    pump_until(&mut map, |m| {
        z1_quadrants().iter().all(|c| {
            m.context()
                .world
                .tiles
                .query::<&VectorLayerBucketComponent>(*c)
                .is_some_and(|b| b.done && b.failed)
        })
    })
    .await;
    let fallback_px = pixel_at(&map, 64, 64);
    assert!(
        fallback_px[0] > 200 && fallback_px[2] < 50,
        "vector HTTP 500 must retain red parent fallback, got {fallback_px:?}"
    );

    client.set_handler(|_| MockResponse::Status(204));
    map.frame_input_mut().advance(Duration::from_millis(2000));
    pump_until(&mut map, |m| {
        z1_quadrants().iter().all(|c| {
            m.context()
                .world
                .tiles
                .query::<&VectorLayerBucketComponent>(*c)
                .is_some_and(|b| b.done && !b.failed)
        })
    })
    .await;
    let recovered_px = pixel_at(&map, 64, 64);
    assert!(
        recovered_px[2] > 200 && recovered_px[0] < 50,
        "retry recovery to 204 should show background blue, got {recovered_px:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn raster_loader_404_triggers_parent_fallback() {
    let green_png = encode_solid_rgba_png(64, 64, [0, 255, 0, 255]);
    let client = MockTileClient::new(move |url| {
        if url.ends_with("/0/0/0.png") {
            MockResponse::Bytes(green_png.clone())
        } else {
            MockResponse::Status(404)
        }
    });
    let mut map = make_loader_map(
        raster_style("http://mock.invalid/{z}/{x}/{y}.png"),
        client,
        true,
    )
    .await;
    pump_until(&mut map, |m| {
        m.context()
            .world
            .tiles
            .query::<&RasterLayersDataComponent>(WorldTileCoords::from((0, 0, 0_u8.into())))
            .is_some_and(|d| d.has_source_image(&"raster_src".into()))
    })
    .await;
    assert!(pixel_at(&map, 64, 64)[1] > 200, "z0 should draw green");

    map.jump_to(CameraOptions {
        zoom: Some(1.0),
        ..Default::default()
    })
    .expect("jump_to z1");
    pump_until(&mut map, |m| {
        z1_quadrants().iter().all(|c| {
            m.context()
                .world
                .tiles
                .query::<&RasterLayersDataComponent>(*c)
                .is_some_and(|d| d.source_is_missing(&"raster_src".into()))
        })
    })
    .await;
    let px = pixel_at(&map, 64, 64);
    assert!(
        px[1] > 200 && px[2] < 50,
        "raster HTTP 404 must render green z0 parent fallback, got {px:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn raster_loader_204_suppresses_parent_fallback_without_gpu_texture() {
    let green_png = encode_solid_rgba_png(64, 64, [0, 255, 0, 255]);
    let client = MockTileClient::new(move |url| {
        if url.ends_with("/0/0/0.png") {
            MockResponse::Bytes(green_png.clone())
        } else {
            MockResponse::Status(204)
        }
    });
    let mut map = make_loader_map(
        raster_style("http://mock.invalid/{z}/{x}/{y}.png"),
        client,
        true,
    )
    .await;
    pump_until(&mut map, |m| {
        m.context()
            .world
            .tiles
            .query::<&RasterLayersDataComponent>(WorldTileCoords::from((0, 0, 0_u8.into())))
            .is_some_and(|d| d.has_source_image(&"raster_src".into()))
    })
    .await;

    map.jump_to(CameraOptions {
        zoom: Some(1.0),
        ..Default::default()
    })
    .expect("jump_to z1");
    pump_until(&mut map, |m| {
        z1_quadrants().iter().all(|c| {
            m.context()
                .world
                .tiles
                .query::<&RasterLayersDataComponent>(*c)
                .is_some_and(|d| d.has_empty_source(&"raster_src".into()))
        })
    })
    .await;
    let px = pixel_at(&map, 64, 64);
    assert!(
        px[2] > 200 && px[1] < 50,
        "raster HTTP 204 must suppress parent fallback, got {px:?}"
    );

    let Eventually::Initialized(raster) = map
        .context()
        .world
        .resources
        .get::<Eventually<RasterResources>>()
        .expect("raster resources")
    else {
        panic!("raster resources not initialized");
    };
    for coords in z1_quadrants() {
        let comp = map
            .context()
            .world
            .tiles
            .query::<&RasterLayersDataComponent>(coords)
            .expect("z1 component");
        assert!(comp
            .layers
            .iter()
            .any(|l| matches!(l, RasterLayerData::Empty(_))));
        assert!(
            raster
                .get_bound_texture(&"raster_src".into(), &coords)
                .is_none(),
            "HTTP 204 child {coords} must allocate zero GPU texture"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mixed_z1_quadrants_render_expected_colors() {
    let green = encode_solid_rgba_png(64, 64, [0, 255, 0, 255]);
    let red = encode_solid_rgba_png(64, 64, [255, 0, 0, 255]);
    let yellow = encode_solid_rgba_png(64, 64, [255, 255, 0, 255]);

    let client = MockTileClient::new(move |url| {
        if url.ends_with("/0/0/0.png") {
            MockResponse::Bytes(green.clone())
        } else if url.ends_with("/1/0/0.png") {
            MockResponse::Bytes(red.clone())
        } else if url.ends_with("/1/1/0.png") {
            MockResponse::Status(204)
        } else if url.ends_with("/1/0/1.png") {
            MockResponse::Status(404)
        } else {
            MockResponse::Bytes(yellow.clone())
        }
    });
    let mut map = make_loader_map(
        raster_style("http://mock.invalid/{z}/{x}/{y}.png"),
        client,
        true,
    )
    .await;
    pump_until(&mut map, |m| {
        m.context()
            .world
            .tiles
            .query::<&RasterLayersDataComponent>(WorldTileCoords::from((0, 0, 0_u8.into())))
            .is_some_and(|d| d.has_source_image(&"raster_src".into()))
    })
    .await;

    map.jump_to(CameraOptions {
        zoom: Some(1.0),
        ..Default::default()
    })
    .expect("jump_to z1");
    pump_until(&mut map, |m| {
        z1_quadrants().iter().all(|c| {
            m.context()
                .world
                .tiles
                .query::<&RasterLayersDataComponent>(*c)
                .is_some_and(|d| d.has_source_result(&"raster_src".into()))
        })
    })
    .await;

    let mut readback = FrameReadback::new(&map, SIZE, SIZE);
    let tl = pixel_with_readback(&map, &mut readback, 64, 64);
    let tr = pixel_with_readback(&map, &mut readback, 192, 64);
    let bl = pixel_with_readback(&map, &mut readback, 64, 192);
    let br = pixel_with_readback(&map, &mut readback, 192, 192);

    assert!(
        tl[0] > 200 && tl[1] < 50 && tl[2] < 50,
        "TL valid child should be red, got {tl:?}"
    );
    assert!(
        tr[2] > 200 && tr[0] < 50 && tr[1] < 50,
        "TR 204 child should show background blue, got {tr:?}"
    );
    assert!(
        bl[1] > 200 && bl[0] < 50 && bl[2] < 50,
        "BL 404 child should fall back to green z0, got {bl:?}"
    );
    assert!(
        br[0] > 200 && br[1] > 200 && br[2] < 50,
        "BR valid child should be yellow, got {br:?}"
    );
}

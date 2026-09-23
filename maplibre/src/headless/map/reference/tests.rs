#![allow(clippy::expect_used, clippy::panic)]

use cgmath::{Matrix4, Vector3};
use image::{Rgba, RgbaImage};

use super::*;
use crate::{
    coords::{LatLon, WorldTileCoords},
    headless::{create_headless_renderer, map::ProcessedLayers, HeadlessPlugin},
    raster::{AvailableRasterLayerData, DefaultRasterTransferables, RasterPlugin},
    render::RenderPlugin,
    style::Style,
    terrain::{DefaultDemTransferables, TerrainPlugin},
};

/// Small targets select a coarser tile zoom than the supplied tile.
const SIZE: u32 = 512;
const TILE: (i32, i32, u8) = (38428, 49355, 17);
/// Terrarium `R*256 + G + B/256 - 32768` for `Rgba(128, 20, 0)`.
const TERRAIN_M: f64 = 20.0;
const CAMERA_M: f64 = 130.0;

fn intrinsics() -> PinholeIntrinsics {
    PinholeIntrinsics {
        width: SIZE,
        height: SIZE,
        fx: 400.0,
        fy: 400.0,
        cx: 255.5,
        cy: 255.5,
    }
}

#[test]
fn centred_intrinsics_give_a_symmetric_frustum() {
    let frustum = intrinsics().frustum();
    assert!((frustum.left - frustum.right).abs() < 1e-12);
    assert!((frustum.top - frustum.bottom).abs() < 1e-12);
    assert!((frustum.left - 0.64).abs() < 1e-12);
    assert_eq!(frustum.near, REFERENCE_NEAR_M);
    assert_eq!(frustum.far, REFERENCE_FAR_M);
}

#[test]
fn reversed_z_maps_to_metres_along_the_axis() {
    assert!((f64::from(optical_depth_m(1.0)) - REFERENCE_NEAR_M).abs() < 1e-4);
    assert_eq!(optical_depth_m(0.0), 0.0);
    assert_eq!(optical_depth_m(f32::NAN), 0.0);
    assert_eq!(optical_depth_m(1.5), 0.0);
    let d = 0.1_f64;
    let expected = REFERENCE_NEAR_M * REFERENCE_FAR_M
        / (REFERENCE_NEAR_M + d * (REFERENCE_FAR_M - REFERENCE_NEAR_M));
    assert!((f64::from(optical_depth_m(0.1)) - expected).abs() / expected < 1e-5);
}

#[test]
fn invalid_intrinsics_are_refused() {
    let mut bad = intrinsics();
    bad.fx = 0.0;
    assert!(matches!(
        bad.validate(),
        Err(ReferenceError::InvalidIntrinsics)
    ));
    bad = intrinsics();
    bad.cy = f64::NAN;
    assert!(matches!(
        bad.validate(),
        Err(ReferenceError::InvalidIntrinsics)
    ));
}

#[tokio::test]
async fn a_nadir_view_reads_the_camera_height_above_terrain() {
    let mut map = prepared_map().await;
    let target = ReferenceTarget::new(&map, intrinsics()).expect("target");
    let render = target
        .render_blocking(
            &mut map,
            anchor(),
            Matrix4::from_translation(Vector3::new(0.0, 0.0, CAMERA_M)),
        )
        .expect("render");
    assert_eq!(render.rgba.len(), (SIZE * SIZE * 4) as usize);
    assert_eq!(render.depth_m.len(), (SIZE * SIZE) as usize);
    let centre = ((SIZE / 2) * SIZE + SIZE / 2) as usize;
    assert_eq!(
        render.rgba[centre * 4 + 3],
        255,
        "imagery covers the centre"
    );
    let depth = f64::from(render.depth_m[centre]);
    assert!(
        (depth - (CAMERA_M - TERRAIN_M)).abs() < 2.0,
        "centre depth {depth} m"
    );
}

#[tokio::test]
async fn a_camera_of_another_size_is_refused() {
    let mut map = prepared_map().await;
    let mut other = intrinsics();
    other.width = SIZE * 2;
    let target = ReferenceTarget::new(&map, other).expect("target");
    let result = target.draw(
        &mut map,
        anchor(),
        Matrix4::from_translation(Vector3::new(0.0, 0.0, CAMERA_M)),
    );
    assert!(matches!(result, Err(ReferenceError::SizeMismatch { .. })));
}

#[tokio::test]
async fn settle_frames_continue_the_map_clock() {
    let mut map = prepared_map().await;
    let before = map.frame_input_mut().timestamp;
    let target = ReferenceTarget::new(&map, intrinsics()).expect("target");
    let eye = Matrix4::from_translation(Vector3::new(0.0, 0.0, CAMERA_M));
    target.draw(&mut map, anchor(), eye).expect("draw");
    let after = map.frame_input_mut().timestamp;
    assert_eq!(
        after,
        before + SETTLE_FRAME_INTERVAL * REFERENCE_SETTLE_FRAMES
    );
}

fn anchor() -> ExternalAnchor {
    let n = 2_f64.powi(i32::from(TILE.2));
    let lon = (f64::from(TILE.0) + 0.5) / n * 360.0 - 180.0;
    let lat = (std::f64::consts::PI * (1.0 - 2.0 * (f64::from(TILE.1) + 0.5) / n))
        .sinh()
        .atan()
        .to_degrees();
    ExternalAnchor {
        position: LatLon::new(lat, lon),
        altitude_meters: 0.0,
    }
}

async fn prepared_map() -> HeadlessMap {
    let style: Style = serde_json::from_value(serde_json::json!({
        "version":8,"projection":{"type":"mercator"},"terrain":{"source":"dem"},
        "sources":{"imagery":{"type":"raster","tiles":["offline://imagery"],"tileSize":512,"maxzoom":17},
        "dem":{"type":"raster-dem","tiles":["offline://dem"],"tileSize":256,"maxzoom":14,"encoding":"terrarium"}},
        "layers":[{"id":"imagery","type":"raster","source":"imagery"}]
    }))
    .expect("style");
    let (kernel, renderer) = create_headless_renderer(SIZE, SIZE, None)
        .await
        .expect("renderer");
    let mut map = HeadlessMap::new(
        style,
        renderer,
        kernel,
        vec![
            Box::new(RenderPlugin),
            Box::new(RasterPlugin::<DefaultRasterTransferables>::default()),
            Box::new(TerrainPlugin::<DefaultDemTransferables>::default()),
            Box::new(
                HeadlessPlugin::new(false)
                    .preserve_tile_sources()
                    .retain_supplied_tiles(),
            ),
        ],
    )
    .expect("map");
    map.render_frames_with_terrain(
        ProcessedLayers::default(),
        vec![AvailableRasterLayerData {
            coords: WorldTileCoords::from((TILE.0, TILE.1, TILE.2.into())),
            source_layer: "imagery".into(),
            image: RgbaImage::from_pixel(512, 512, Rgba([90, 120, 150, 255])),
        }],
        vec![(
            WorldTileCoords::from((TILE.0 / 8, TILE.1 / 8, 14_u8.into())),
            RgbaImage::from_pixel(256, 256, Rgba([128, 20, 0, 255])),
        )],
        3,
    )
    .expect("tiles");
    map
}

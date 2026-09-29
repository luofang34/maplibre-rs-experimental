//! Cached terrain pixels follow the uniforms actually supplied to DEM shading.

use super::*;
use crate::{
    style::{
        hillshade::IlluminationAnchor,
        layer::{LayerPaint, StyleProperty},
    },
    terrain::DrapePhase,
};

#[tokio::test]
async fn relief_opacity_changes_refresh_cached_pixels_without_source_uploads() {
    let mut map = prepared_map(true).await;
    assert_color(
        &read_blocking(&map, "uniform-relief-before"),
        [8, 136, 8, 255],
    );
    let Some(LayerPaint::ColorRelief(paint)) = &mut map.map_context.style.layers[1].paint else {
        panic!("relief paint");
    };
    paint.color_relief_opacity = Some(StyleProperty::Constant(0.0));
    map.run_frame().expect("changed paint frame");
    assert_color(
        &read_blocking(&map, "uniform-relief-after"),
        [16, 16, 16, 255],
    );
    assert_redrawn(&map, true);
    map.run_frame().expect("unchanged paint frame");
    assert_redrawn(&map, false);
}

#[tokio::test]
async fn viewport_light_bearing_refreshes_drapes_but_map_light_reuses_them() {
    let mut map = hillshade_map(0.0).await;
    map.map_context
        .view_state
        .camera_mut()
        .set_bearing(cgmath::Deg(90.0));
    map.run_frame().expect("rotated viewport light");
    let cold = hillshade_map(90.0).await;
    let warm_pixels = read_blocking(&map, "uniform-light-warm");
    let cold_pixels = read_blocking(&cold, "uniform-light-cold");
    let mismatch = warm_pixels
        .chunks_exact(4)
        .zip(cold_pixels.chunks_exact(4))
        .enumerate()
        .find(|(_, (warm, cold))| warm != cold)
        .map(|(index, (warm, cold))| (index % SIZE as usize, index / SIZE as usize, warm, cold));
    assert!(
        mismatch.is_none(),
        "cached slope differs from fresh shading: {mismatch:?}"
    );
    assert_redrawn(&map, true);
    map.run_frame().expect("unchanged viewport light");
    assert_redrawn(&map, false);

    let Some(LayerPaint::Hillshade(paint)) = &mut map.map_context.style.layers[1].paint else {
        panic!("hillshade paint");
    };
    paint.hillshade_illumination_anchor = IlluminationAnchor::Map;
    map.run_frame().expect("map light");
    map.map_context
        .view_state
        .camera_mut()
        .set_bearing(cgmath::Deg(0.0));
    map.run_frame().expect("rotated camera with map light");
    assert_redrawn(&map, false);
}

async fn hillshade_map(bearing: f64) -> HeadlessMap {
    let mut map = prepared_map(true).await;
    map.map_context
        .view_state
        .camera_mut()
        .set_bearing(cgmath::Deg(bearing));
    let mut layer = serde_json::from_value::<crate::style::layer::StyleLayer>(
        serde_json::json!({"id":"paint","type":"hillshade","source":"paint",
            "paint":{"hillshade-illumination-anchor":"viewport",
                "hillshade-illumination-direction":0,"hillshade-exaggeration":1.0}}),
    )
    .expect("hillshade layer");
    layer.index = 1;
    map.map_context.style.layers[1] = layer;
    let mut slope = tile(target(), true, false);
    slope.image = RgbaImage::from_fn(256, 256, |x, _| {
        let encoded = 32768 + x * 16;
        Rgba([(encoded >> 8) as u8, encoded as u8, 0, 255])
    });
    map.render_sources(ProcessedLayers::default(), vec![slope])
        .expect("uploaded slope");
    map.run_frame().expect("stable slope");
    assert_redrawn(&map, false);
    map
}

fn assert_redrawn(map: &HeadlessMap, expected: bool) {
    let phase = map
        .world()
        .resources
        .get::<DrapePhase>()
        .expect("drape phase");
    assert_eq!(
        phase.targets.iter().any(|item| item.coords == target()),
        expected
    );
}

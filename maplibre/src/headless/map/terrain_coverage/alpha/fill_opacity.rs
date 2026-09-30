//! `fill-opacity` blends a fill with what lies under it, on the screen and on terrain.
use super::*;

async fn blended_center(terrain: bool, samples: u32, opacity: serde_json::Value) -> Vec<u8> {
    let mut style = alpha_style(terrain, 1.0, None);
    style.layers[0] = fill_layer(opacity);
    let style = with_background(style, "#ffffff");
    let map = styled_alpha_map(style, samples, None, true).await;
    pixels_blocking(&map, "fill-opacity")
}

#[tokio::test]
async fn fill_opacity_blends_with_the_background_in_every_covered_mode() {
    for terrain in [false, true] {
        for samples in [1, 4] {
            for (opacity, expected) in [
                (serde_json::json!(1.0), [0, 0, 255, 255]),
                (serde_json::json!(0.0), [255, 255, 255, 255]),
                (serde_json::json!(0.5), [128, 128, 255, 255]),
                // Unset means opaque, and values beyond the range are clamped to it.
                (serde_json::Value::Null, [0, 0, 255, 255]),
                (serde_json::json!(2.0), [0, 0, 255, 255]),
                (serde_json::json!(-1.0), [255, 255, 255, 255]),
                // Per-feature values take the vertex-colour path instead of the layer uniform.
                (serde_json::json!(["get", "alpha"]), [128, 128, 255, 255]),
                (serde_json::json!(["get", "missing"]), [0, 0, 255, 255]),
            ] {
                let bytes = blended_center(terrain, samples, opacity.clone()).await;
                assert_center(&bytes, expected);
            }
        }
    }
}

/// A layer with `fill-opacity` set, or left at its default when the value is null.
fn fill_layer(opacity: serde_json::Value) -> crate::style::layer::StyleLayer {
    let mut paint = serde_json::json!({"fill-color":"#0000ff"});
    if !opacity.is_null() {
        paint["fill-opacity"] = opacity;
    }
    serde_json::from_value(serde_json::json!({
        "id":"fill", "type":"fill", "source":"vector", "source-layer":"fill", "paint":paint
    }))
    .expect("fill")
}

#[tokio::test]
async fn a_zoom_expression_blends_with_the_opacity_of_the_view_zoom() {
    let opacity = serde_json::json!(["interpolate", ["linear"], ["zoom"], 0, 0, 24, 1]);
    let mut style = alpha_style(false, 1.0, None);
    style.layers[0] = fill_layer(opacity);
    let map = styled_alpha_map(with_background(style, "#ffffff"), 1, None, true).await;
    let zoom = map.view_state().zoom().value();
    let channel = (255.0 * (1.0 - zoom / 24.0)).round() as u8;
    assert_center(
        &pixels_blocking(&map, "fill-opacity-zoom"),
        [channel, channel, 255, 255],
    );
}

#[tokio::test]
async fn fill_opacity_blends_on_the_globe() {
    let mut style = globe::globe_style(target());
    style.layers = vec![fill_layer(serde_json::json!(0.5))];
    let style = with_background(style, "#ffffff");
    let map = styled_alpha_map(style, 1, None, true).await;
    globe::assert_spherical(&map);
    assert_center(
        &pixels_blocking(&map, "fill-opacity-globe"),
        [128, 128, 255, 255],
    );
}

#[tokio::test]
async fn color_alpha_multiplies_with_fill_opacity() {
    let mut style = alpha_style(false, 1.0, None);
    style.layers[0] = serde_json::from_value(serde_json::json!({
        "id":"fill", "type":"fill", "source":"vector", "source-layer":"fill",
        "paint":{"fill-color":"rgba(0, 0, 255, 0.5)", "fill-opacity":0.5}
    }))
    .expect("fill");
    let map = styled_alpha_map(with_background(style, "#ffffff"), 1, None, true).await;
    assert_center(
        &pixels_blocking(&map, "fill-opacity-color-alpha"),
        [191, 191, 255, 255],
    );
}

#![allow(clippy::expect_used, clippy::panic)]
use super::*;

#[tokio::test]
async fn shared_height_uses_display_zoom_even_for_parent_symbol_geometry() {
    let mut expected = style(100., "ground");
    expected.zoom = Some(12.5);
    let expected_pixels = read_blocking(&fixture_map(expected.clone(), layers(&expected), 1).await);
    let mut value = serde_json::to_value(expected).expect("style");
    value["layers"][2]["layout"]["symbol-height-offset"] =
        serde_json::json!(["interpolate", ["linear"], ["zoom"], 12, 0, 13, 200]);
    let expression: Style = serde_json::from_value(value).expect("zoom height");
    let actual = read_blocking(&fixture_map(expression.clone(), layers(&expression), 1).await);
    assert!(colored_bounds(&actual, 0).0 > 70);
    assert_eq!(
        actual, expected_pixels,
        "parent tile zoom must not fix the symbol's height"
    );
}

#[tokio::test]
async fn rotated_text_keeps_antialiased_edges_without_msaa() {
    for angle in [0, 30, 60] {
        let mut value = serde_json::to_value(style(0., "ground")).expect("style");
        value["layers"][0]["paint"]["background-color"] = "#000000".into();
        value["layers"][2]["layout"]["text-rotate"] = angle.into();
        value["layers"][2]["paint"]["text-halo-width"] = 0.into();
        let style: Style = serde_json::from_value(value).expect("rotation");
        let pixels = read_blocking(&fixture_map(style.clone(), layers(&style), 1).await);
        assert!(
            colored_bounds(&pixels, 0).0 > 70,
            "stroke contrast at {angle} degrees"
        );
        let edges = pixels
            .chunks_exact(4)
            .filter(|p| p[0] > 110 && p[0] < 230 && p[1] < 70)
            .count();
        assert!(edges > 20, "subpixel coverage at {angle} degrees: {edges}");
    }
}

#[tokio::test]
async fn zero_width_halo_does_not_tint_antialiased_text_edges() {
    let mut value = serde_json::to_value(style(0., "ground")).expect("style");
    value["layers"][2]["paint"]["text-halo-width"] = 0.into();
    let opaque: Style = serde_json::from_value(value.clone()).expect("opaque halo");
    let opaque_pixels = read_blocking(&fixture_map(opaque.clone(), layers(&opaque), 1).await);
    value["layers"][2]["paint"]["text-halo-color"] = "transparent".into();
    let clear: Style = serde_json::from_value(value).expect("clear halo");
    let clear_pixels = read_blocking(&fixture_map(clear.clone(), layers(&clear), 1).await);
    assert_eq!(
        opaque_pixels, clear_pixels,
        "zero-width halo must not thicken the glyph edges"
    );
}

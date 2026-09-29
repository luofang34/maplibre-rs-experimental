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

fn application_label(id: &str, zoom: f64) -> Style {
    let app: Style = serde_json::from_str(include_str!(
        "../../../../../apple/visionos/MapLibreVision/MapLibreVision/style.json"
    ))
    .expect("application style");
    let layer = app
        .layers
        .iter()
        .find(|layer| layer.id == id)
        .expect("place");
    let Some(LayerPaint::Symbol(paint)) = &layer.paint else {
        panic!("place symbol");
    };
    let size = paint
        .text_size
        .as_ref()
        .expect("size")
        .evaluate_at_zoom(zoom)
        .expect("size value");
    let mut value = serde_json::to_value(style(0., "ground")).expect("fixture");
    value["pitch"] = 0.into();
    value["layers"][0]["paint"]["background-color"] = "#889988".into();
    let label = &mut value["layers"][2];
    label["layout"]["text-size"] = size.into();
    label["layout"]
        .as_object_mut()
        .expect("layout")
        .remove("icon-image");
    label["paint"] = serde_json::to_value(paint).expect("paint");
    // The fixed tile and bundled glyph atlas isolate paint from network and camera coverage.
    label["layout"]["text-font"] = serde_json::json!(["Open Sans Regular"]);
    serde_json::from_value(value).expect("application label fixture")
}

#[tokio::test]
async fn application_halos_preserve_strokes_at_small_large_and_intermediate_sizes() {
    let capture = std::env::var_os("MAPLIBRE_TEST_CAPTURE_DIR").map(std::path::PathBuf::from);
    for (id, zoom) in [
        ("place_other", 14.),
        ("place_other", 16.5),
        ("place_other", 19.),
        ("place_city_large", 5.),
        ("place_city_large", 7.5),
        ("place_city_large", 10.),
    ] {
        let style = application_label(id, zoom);
        let actual = assert_halo_preserves_strokes(style, &format!("{id} at {zoom}")).await;
        if let Some(path) = &capture {
            std::fs::create_dir_all(path).expect("capture directory");
            image::save_buffer(
                path.join(format!("{id}-{zoom}.png")),
                &actual,
                SIZE,
                SIZE,
                image::ColorType::Rgba8,
            )
            .expect("label capture");
        }
    }
}

async fn assert_halo_preserves_strokes(style: Style, case: &str) -> Vec<u8> {
    let actual = read_blocking(&fixture_map(style.clone(), layers(&style), 1).await);
    let mut clear = serde_json::to_value(style).expect("style");
    clear["layers"][2]["paint"]["text-halo-width"] = 0.into();
    let clear: Style = serde_json::from_value(clear).expect("no halo");
    let baseline = read_blocking(&fixture_map(clear.clone(), layers(&clear), 1).await);
    let mut strokes = 0_usize;
    let mut halo_pixels = 0_usize;
    for (with_halo, without) in actual.chunks_exact(4).zip(baseline.chunks_exact(4)) {
        if without[..3] == [36, 55, 64] {
            strokes = strokes.wrapping_add(1);
            // Rounded RGBA8 fill colors can include a fractionally covered edge pixel.
            assert!(
                with_halo
                    .iter()
                    .zip(without)
                    .all(|(a, b)| a.abs_diff(*b) <= 1),
                "halo covers interior: {case}: {with_halo:?} vs {without:?}"
            );
        }
        if with_halo[..3]
            .iter()
            .zip(without)
            .any(|(a, b)| a.abs_diff(*b) > 20)
        {
            halo_pixels = halo_pixels.wrapping_add(1);
        }
    }
    assert!(strokes > 0, "missing interior strokes: {case}: {strokes}");
    assert!(halo_pixels > 20, "missing halo: {case}: {halo_pixels}");
    actual
}

#[tokio::test]
async fn negative_letter_spacing_keeps_halos_below_adjacent_strokes() {
    let mut value = serde_json::to_value(application_label("place_other", 19.)).expect("style");
    value["layers"][2]["layout"]["text-letter-spacing"] = (-0.2).into();
    let style: Style = serde_json::from_value(value).expect("overlapping glyphs");
    assert_halo_preserves_strokes(style, "negative letter spacing").await;
}

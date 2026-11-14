#![allow(clippy::expect_used, clippy::panic)]

use super::{HillshadeMethod, HillshadePaint, IlluminationAnchor};
use crate::style::layer::{LayerPaint, StyleLayer};

fn hillshade(json: serde_json::Value) -> HillshadePaint {
    let layer: StyleLayer = serde_json::from_value(json).expect("layer parses");
    match layer.paint {
        Some(LayerPaint::Hillshade(paint)) => paint,
        other => panic!("expected hillshade paint, got {other:?}"),
    }
}

#[test]
fn a_hillshade_layer_without_paint_takes_the_specification_defaults() {
    let paint = hillshade(serde_json::json!({"id": "h", "type": "hillshade", "source": "dem"}));
    let lights = paint.illumination(11.0, 0.0);
    assert_eq!(paint.hillshade_method, HillshadeMethod::Standard);
    assert_eq!(
        paint.hillshade_illumination_anchor,
        IlluminationAnchor::Viewport
    );
    assert_eq!(lights.azimuths.len(), 1);
    assert!((lights.azimuths[0] - 335.0_f32.to_radians()).abs() < 1e-6);
    assert!((lights.altitudes[0] - 45.0_f32.to_radians()).abs() < 1e-6);
    assert_eq!(lights.shadows[0], [0.0, 0.0, 0.0, 1.0]);
    assert_eq!(lights.highlights[0], [1.0, 1.0, 1.0, 1.0]);
    assert_eq!(paint.exaggeration_at(11.0), 0.5);
}

#[test]
fn several_lights_pad_every_list_to_the_longest_and_turn_with_the_viewport() {
    let paint = hillshade(serde_json::json!({
        "id": "h", "type": "hillshade", "source": "dem",
        "paint": {
            "hillshade-method": "multidirectional",
            "hillshade-illumination-direction": [270, 315, 0, 45],
            "hillshade-highlight-color": ["#FF4000", "#FFFF00"],
            "hillshade-shadow-color": "#00bfff"
        }
    }));
    let lights = paint.illumination(11.0, 0.5);
    assert_eq!(paint.hillshade_method, HillshadeMethod::Multidirectional);
    assert_eq!(lights.azimuths.len(), 4);
    assert!(
        (lights.azimuths[2] - 0.5).abs() < 1e-6,
        "north turned by the bearing"
    );
    assert_eq!(lights.highlights.len(), 4);
    assert_eq!(
        lights.highlights[3], lights.highlights[1],
        "the last colour repeats"
    );
    assert_eq!(lights.shadows.len(), 4);
    assert!((lights.shadows[0][2] - 1.0).abs() < 1e-6);
}

#[test]
fn zoom_functions_drive_the_colours() {
    let paint = hillshade(serde_json::json!({
        "id": "h", "type": "hillshade", "source": "dem",
        "paint": {"hillshade-accent-color": {"stops": [[10, "#000000"], [12, "#ffffff"]]}}
    }));
    let accent = paint.accent_at(11.0);
    assert!((accent[0] - 0.5).abs() < 1e-6, "{accent:?}");
}

#[test]
fn a_relief_ramp_lists_the_elevation_stops_with_their_colours() {
    let layer: StyleLayer = serde_json::from_value(serde_json::json!({
        "id": "r", "type": "color-relief", "source": "dem",
        "paint": {"color-relief-color": ["interpolate", ["linear"], ["elevation"], 400, "#F00", 800, "#00F"]}
    }))
    .expect("layer parses");
    let Some(LayerPaint::ColorRelief(paint)) = layer.paint else {
        panic!("expected color-relief paint");
    };
    let ramp = paint.ramp();
    assert_eq!(ramp.len(), 2);
    assert_eq!(ramp[0], (400.0, [1.0, 0.0, 0.0, 1.0]));
    assert_eq!(ramp[1], (800.0, [0.0, 0.0, 1.0, 1.0]));
    assert_eq!(paint.opacity_at(3.0), 1.0);
    assert!(!paint.is_nearest_at(3.0));
}

#[test]
fn hillshade_and_color_relief_resampling_support_constant_stops_and_mutation() {
    let default_hs =
        hillshade(serde_json::json!({"id": "h", "type": "hillshade", "source": "dem"}));
    assert!(!default_hs.is_nearest_at(14.0));

    let stop_hs = hillshade(serde_json::json!({
        "id": "h",
        "type": "hillshade",
        "source": "dem",
        "paint": {
            "hillshade-resampling": { "stops": [[10.0, "linear"], [14.0, "nearest"]] }
        }
    }));
    assert!(!stop_hs.is_nearest_at(12.0));
    assert!(stop_hs.is_nearest_at(14.0));
    assert!(stop_hs.is_nearest_at(18.0));

    let mut style: crate::style::Style = serde_json::from_value(serde_json::json!({
        "version": 8,
        "sources": {
            "dem": { "type": "raster-dem", "tiles": ["https://example.test/{z}/{x}/{y}.png"] }
        },
        "layers": [
            { "id": "hs", "type": "hillshade", "source": "dem" },
            { "id": "cr", "type": "color-relief", "source": "dem", "paint": { "color-relief-resampling": "nearest" } }
        ]
    }))
    .expect("style parses");
    match &style.layers[1].paint {
        Some(LayerPaint::ColorRelief(cr)) => assert!(cr.is_nearest_at(5.0)),
        other => panic!("expected color-relief paint, got {other:?}"),
    }
    let change = style
        .set_paint_property("hs", "hillshade-resampling", serde_json::json!("nearest"))
        .expect("hillshade-resampling mutates");
    assert_eq!(change.layers, vec!["hs".to_string()]);
    assert!(!change.redraw_tiles);
    match &style.layers[0].paint {
        Some(LayerPaint::Hillshade(hs)) => assert!(hs.is_nearest_at(5.0)),
        other => panic!("expected hillshade paint, got {other:?}"),
    }
}

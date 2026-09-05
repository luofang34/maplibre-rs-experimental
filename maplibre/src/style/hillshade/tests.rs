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
}

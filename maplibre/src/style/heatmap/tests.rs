#![allow(clippy::expect_used, clippy::panic)]
use super::*;

fn paint(json: serde_json::Value) -> HeatmapPaint {
    serde_json::from_value(json).expect("heatmap paint")
}

#[test]
fn absent_properties_take_the_specification_defaults() {
    let paint = HeatmapPaint::default();
    assert_eq!(paint.radius_at(5.0), 30.0);
    assert_eq!(paint.intensity_at(5.0), 1.0);
    assert_eq!(paint.opacity_at(5.0), 1.0);
    let ramp = paint.ramp();
    assert_eq!(ramp.len(), RAMP_TEXELS);
    assert_eq!(ramp[0], [0, 0, 0, 0], "zero density is transparent");
    assert_eq!(
        ramp[RAMP_TEXELS - 1],
        [255, 0, 0, 255],
        "full density is red"
    );
}

#[test]
fn zoom_expressions_drive_radius_intensity_and_opacity() {
    let paint = paint(serde_json::json!({
        "heatmap-radius": ["interpolate", ["linear"], ["zoom"], 0, 10, 10, 30],
        "heatmap-intensity": ["interpolate", ["linear"], ["zoom"], 0, 1, 10, 3],
        "heatmap-opacity": 0.5
    }));
    assert_eq!(paint.radius_at(5.0), 20.0);
    assert_eq!(paint.intensity_at(5.0), 2.0);
    assert_eq!(paint.opacity_at(5.0), 0.5);
}

#[test]
fn a_ramp_reproduces_its_stops_and_a_step_keeps_its_edge() {
    let interpolated = paint(serde_json::json!({
        "heatmap-color": ["interpolate", ["linear"], ["heatmap-density"],
            0, "rgba(0, 0, 0, 0)", 1, "rgb(255, 0, 0)"]
    }))
    .ramp();
    assert_eq!(interpolated[0], [0, 0, 0, 0]);
    assert_eq!(interpolated[RAMP_TEXELS - 1], [255, 0, 0, 255]);
    assert!(
        interpolated[128][3] > 120 && interpolated[128][3] < 136,
        "{:?}",
        interpolated[128]
    );
    let stepped = paint(serde_json::json!({
        "heatmap-color": ["step", ["heatmap-density"], "rgb(0, 0, 255)", 0.5, "rgb(255, 0, 0)"]
    }))
    .ramp();
    assert_eq!(stepped[100], [0, 0, 255, 255]);
    assert_eq!(stepped[200], [255, 0, 0, 255]);
}

#[test]
fn an_unusable_colour_falls_back_to_the_default_ramp() {
    let broken = paint(serde_json::json!({"heatmap-color": ["get", "colour"]})).ramp();
    assert_eq!(broken, HeatmapPaint::default().ramp());
}

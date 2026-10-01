#![allow(clippy::expect_used, clippy::panic)]
use super::*;

fn gradient(value: serde_json::Value) -> StyleProperty<Color> {
    StyleProperty::parse(&value)
}

#[test]
fn the_ramp_runs_from_the_first_stop_to_the_last() {
    let ramp = ramp(&gradient(serde_json::json!([
        "interpolate",
        ["linear"],
        ["line-progress"],
        0,
        "red",
        1,
        "blue"
    ])));
    assert_eq!(ramp.len(), RAMP_TEXELS);
    assert_eq!(ramp[0], [255, 0, 0, 255]);
    assert_eq!(ramp[RAMP_TEXELS - 1], [0, 0, 255, 255]);
    assert!((i32::from(ramp[128][0]) - 127).abs() <= 2);
}

#[test]
fn a_step_gradient_changes_colour_at_its_stop() {
    let ramp = ramp(&gradient(serde_json::json!([
        "step",
        ["line-progress"],
        "red",
        0.5,
        "blue"
    ])));
    assert_eq!(ramp.len(), STEP_RAMP_TEXELS);
    assert_eq!(ramp[100], [255, 0, 0, 255]);
    assert_eq!(ramp[STEP_RAMP_TEXELS - 100], [0, 0, 255, 255]);
}

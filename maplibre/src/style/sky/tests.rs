#![allow(clippy::expect_used, clippy::panic)]

use super::SkySpecification;

#[test]
fn atmosphere_blend_defaults_to_zero_and_clamps() {
    let missing = SkySpecification::default();
    let excessive: SkySpecification =
        serde_json::from_str(r#"{"atmosphere-blend":2}"#).expect("sky should deserialize");

    assert_eq!(missing.atmosphere_blend_at_zoom(0.0), 0.0);
    assert_eq!(excessive.atmosphere_blend_at_zoom(0.0), 1.0);
}

#[test]
fn atmosphere_blend_interpolates_camera_zoom() {
    let sky: SkySpecification = serde_json::from_str(
        r#"{"atmosphere-blend":["interpolate",["linear"],["zoom"],0,1,10,0]}"#,
    )
    .expect("sky expression should deserialize");

    assert_eq!(sky.atmosphere_blend_at_zoom(0.0), 1.0);
    assert_eq!(sky.atmosphere_blend_at_zoom(5.0), 0.5);
    assert_eq!(sky.atmosphere_blend_at_zoom(10.0), 0.0);
}

#[test]
fn atmosphere_blend_matches_gl_js_transition_fixture() {
    let sky: SkySpecification = serde_json::from_str(
        r#"{"atmosphere-blend":["interpolate",["linear"],["zoom"],0,1,10,1,12,0]}"#,
    )
    .expect("sky expression should deserialize");

    assert_eq!(sky.atmosphere_blend_at_zoom(10.0), 1.0);
    assert_eq!(sky.atmosphere_blend_at_zoom(11.0), 0.5);
    assert_eq!(sky.atmosphere_blend_at_zoom(12.0), 0.0);
}

#[test]
fn atmosphere_blend_step_uses_latest_stop() {
    let sky: SkySpecification =
        serde_json::from_str(r#"{"atmosphere-blend":["step",["zoom"],0,3,0.5,6,1]}"#)
            .expect("sky expression should deserialize");

    assert_eq!(sky.atmosphere_blend_at_zoom(2.0), 0.0);
    assert_eq!(sky.atmosphere_blend_at_zoom(3.0), 0.5);
    assert_eq!(sky.atmosphere_blend_at_zoom(7.0), 1.0);
}

#[test]
fn sky_colours_take_the_specification_defaults_and_zoom_functions() {
    let sky: SkySpecification = serde_json::from_str(
        r##"{"sky-color": "#199EF3", "fog-ground-blend": {"stops": [[10, 0], [12, 1]]}}"##,
    )
    .expect("sky parses");
    let colors = sky.colors_at(11.0);
    assert!(
        (colors.sky[0] - 0x19 as f32 / 255.0).abs() < 1e-6,
        "{colors:?}"
    );
    assert_eq!(colors.horizon, [1.0, 1.0, 1.0, 1.0]);
    assert_eq!(colors.fog, [1.0, 1.0, 1.0, 1.0]);
    assert!((colors.fog_ground_blend - 0.5).abs() < 1e-6, "{colors:?}");
    assert_eq!(colors.horizon_fog_blend, 0.8);
    assert_eq!(colors.sky_horizon_blend, 0.8);
    assert_eq!(SkySpecification::fog_blend_opacity(59.0), 0.0);
    assert!((SkySpecification::fog_blend_opacity(65.0) - 0.5).abs() < 1e-6);
    assert_eq!(SkySpecification::fog_blend_opacity(85.0), 1.0);
}

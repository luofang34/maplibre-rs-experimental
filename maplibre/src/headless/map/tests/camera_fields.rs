//! The style's standard camera fields reach every camera as the specification defines them,
//! and a style is written back with nothing but standard root fields.

use cgmath::{Deg, InnerSpace, Point2, Vector4};

use super::initial_view_state;
use crate::{
    coords::{LatLon, WorldCoords},
    render::{projection::globe_camera_for_view, view_state::ViewState},
    style::Style,
    window::PhysicalSize,
};

/// The root properties of the style specification's `v8.json`, tag v26.4.0.
const ROOT_KEYS: [&str; 20] = [
    "version",
    "name",
    "metadata",
    "center",
    "centerAltitude",
    "zoom",
    "bearing",
    "pitch",
    "roll",
    "state",
    "light",
    "sky",
    "projection",
    "terrain",
    "sources",
    "sprite",
    "glyphs",
    "font-faces",
    "transition",
    "layers",
];

fn style(json: &str) -> Style {
    serde_json::from_str(json).expect("style")
}

#[test]
fn a_style_is_written_back_with_its_camera_fields_and_only_standard_root_fields() {
    let written = serde_json::to_value(style(
        r#"{"version":8,"center":[88.05,27.77],"centerAltitude":4200.5,"zoom":11.67,
            "bearing":324.04,"pitch":70,"roll":-12.5,"vertical-field-of-view":50,
            "sources":{},"layers":[]}"#,
    ))
    .expect("serialize");
    let root = written.as_object().expect("object");
    for (key, value) in root {
        assert!(
            ROOT_KEYS.contains(&key.as_str()),
            "non-standard root field {key}"
        );
        assert!(!value.is_null(), "{key} is written as null");
    }
    assert_eq!(root["center"], serde_json::json!([88.05, 27.77]));
    assert_eq!(root["centerAltitude"], serde_json::json!(4200.5));
    assert_eq!(root["zoom"], serde_json::json!(11.67));
    assert_eq!(root["bearing"], serde_json::json!(324.04));
    assert_eq!(root["pitch"], serde_json::json!(70.0));
    assert_eq!(root["roll"], serde_json::json!(-12.5));
    // A bare style writes no camera fields at all.
    let bare = serde_json::to_value(style(r#"{"version":8,"sources":{},"layers":[]}"#))
        .expect("serialize");
    for key in [
        "center",
        "centerAltitude",
        "zoom",
        "bearing",
        "pitch",
        "roll",
        "projection",
    ] {
        assert!(
            bare.get(key).is_none(),
            "{key} written for a style without it"
        );
    }
}

#[test]
fn the_initial_view_takes_roll_and_center_altitude_and_a_field_of_view_only_at_runtime() {
    let style = style(
        r#"{"version":8,"center":[88.05,27.77],"centerAltitude":4200.5,"zoom":11.67,
            "roll":-12.5,"vertical-field-of-view":50,"sources":{},"layers":[]}"#,
    );
    let view = initial_view_state(PhysicalSize::new(800, 600).expect("size"), &style);
    assert!((view.camera().get_roll().0.to_degrees() + 12.5).abs() < 1e-12);
    assert_eq!(view.center_altitude(), 4200.5);
    assert_eq!(view.center_elevation(), 4200.5);
    assert!(
        view.center_clamped_to_ground(),
        "terrain holds the center by default"
    );
    let default_fov = 0.643_501_108_793_284_4;
    assert!(
        (view.field_of_view().0 - default_fov).abs() < 1e-15,
        "a style field set it"
    );
    let mut view = view;
    view.set_field_of_view(Deg(50.0).into());
    assert!((view.field_of_view().0.to_degrees() - 50.0).abs() < 1e-12);
}

/// The viewport pixel of `location` on the ground through the flat camera.
fn flat_pixel(view: &ViewState, location: LatLon) -> Point2<f64> {
    let world = WorldCoords::from_lat_lon(location, view.zoom());
    let clip = view
        .view_projection()
        .project(Vector4::new(world.x, world.y, 0.0, 1.0));
    Point2::new(
        (clip.x / clip.w * 0.5 + 0.5) * view.width(),
        (-clip.y / clip.w * 0.5 + 0.5) * view.height(),
    )
}

#[test]
fn roll_turns_the_globe_camera_as_it_turns_the_flat_one() {
    for roll in [0.0, 30.0, -75.0, 180.0] {
        let style = style(&format!(
            r#"{{"version":8,"center":[88.05,27.77],"zoom":16,"bearing":20,"pitch":40,
                "roll":{roll},"sources":{{}},"layers":[]}}"#
        ));
        let view = initial_view_state(PhysicalSize::new(800, 600).expect("size"), &style);
        let globe = globe_camera_for_view(&view).expect("camera");
        for (east, north) in [(150.0, 0.0), (0.0, 150.0), (-120.0, -90.0)] {
            let location = LatLon::new(
                27.77 + north / 111_195.0,
                88.05 + east / (111_195.0 * 27.77_f64.to_radians().cos()),
            );
            let flat = flat_pixel(&view, location);
            let round = globe.location_to_screen(location, 0.0);
            assert!(
                (flat - round).magnitude() < 1.0,
                "roll {roll}: ({east},{north}) at {flat:?} flat, {round:?} on the globe"
            );
        }
    }
}

#[tokio::test]
async fn a_center_altitude_holds_without_terrain_and_the_pure_globe_orbits_it() {
    // With the render plugin's own stages, and with the headless plugin that replaces them.
    for headless in [false, true] {
        let style = style(
            r#"{"version":8,"center":[88.05,27.77],"centerAltitude":4200.5,"zoom":11.67,"pitch":60,
                "projection":{"type":"vertical-perspective"},"sources":{},"layers":[]}"#,
        );
        let (kernel, renderer) = crate::headless::create_headless_renderer(64, 64, None)
            .await
            .expect("renderer");
        let mut plugins: Vec<Box<dyn crate::plugin::Plugin<_>>> =
            vec![Box::new(crate::render::RenderPlugin)];
        if headless {
            plugins.push(Box::new(crate::headless::HeadlessPlugin::new(false)));
        }
        let mut map =
            super::super::HeadlessMap::new(style, renderer, kernel, plugins).expect("map");
        // Whatever moved the center, each frame rests it at its altitude again.
        map.map_context.view_state.set_center_elevation(0.0);
        for _ in 0..3 {
            map.run_frame().expect("frame");
        }
        assert_eq!(
            map.view_state().center_elevation(),
            4200.5,
            "headless {headless}"
        );
        let camera = globe_camera_for_view(map.view_state()).expect("camera");
        let expected = camera.body().unit_radius_at(4200.5);
        assert!(
            (camera.target().magnitude() - expected).abs() < 1e-12,
            "headless {headless}: the globe camera orbits sea level"
        );
        map.set_vertical_field_of_view(Deg(50.0));
        assert!((map.view_state().field_of_view().0.to_degrees() - 50.0).abs() < 1e-12);
    }
}

/// The screen direction, as an angle, in which the ground's up points at `location`: from the
/// point at `elevation` to the one a kilometre above it.
fn up_on_screen(project: impl Fn(f64) -> Point2<f64>, elevation: f64) -> f64 {
    let (low, high) = (project(elevation), project(elevation + 1000.0));
    (high.y - low.y).atan2(high.x - low.x)
}

#[test]
fn roll_and_center_altitude_together_raise_the_target_and_tilt_the_horizon() {
    let mut tilts = Vec::new();
    for roll in [0.0, 30.0, -75.0] {
        let style = style(&format!(
            r#"{{"version":8,"center":[88.05,27.77],"centerAltitude":4200.5,"zoom":16,
                "bearing":20,"pitch":40,"roll":{roll},
                "projection":{{"type":"vertical-perspective"}},"sources":{{}},"layers":[]}}"#
        ));
        let mut view = initial_view_state(PhysicalSize::new(800, 600).expect("size"), &style);
        view.set_globe_orbits_center(true);
        let globe = globe_camera_for_view(&view).expect("camera");
        let center = LatLon::new(27.77, 88.05);
        // The raised target is the point at the viewport's center.
        let target = globe.location_to_screen(center, 4200.5);
        assert!(
            (target - Point2::new(400.0, 600.0 / 2.0)).magnitude() < 1e-6,
            "roll {roll}: the target shows at {target:?}"
        );
        let tilt = up_on_screen(|meters| globe.location_to_screen(center, meters), 4200.5);
        tilts.push(tilt);
    }
    // The horizon tilts with the roll, the same amount either way, and counterclockwise for
    // a positive roll of the camera turns the scene clockwise on screen.
    let turned = |index: usize| {
        let turn = (tilts[index] - tilts[0]).to_degrees();
        (turn + 540.0).rem_euclid(360.0) - 180.0
    };
    assert!((turned(1).abs() - 30.0).abs() < 1e-6, "{tilts:?}");
    assert!((turned(2).abs() - 75.0).abs() < 1e-6, "{tilts:?}");
    assert!(turned(1).signum() != turned(2).signum(), "{tilts:?}");
}

#[tokio::test]
async fn whether_terrain_holds_the_center_follows_the_style_frame_by_frame() {
    let (kernel, renderer) = crate::headless::create_headless_renderer(64, 64, None)
        .await
        .expect("renderer");
    let mut map = super::super::HeadlessMap::new(
        style(r#"{"version":8,"centerAltitude":3000,"sources":{},"layers":[]}"#),
        renderer,
        kernel,
        vec![Box::new(crate::render::RenderPlugin)],
    )
    .expect("map");
    map.run_frame().expect("frame");
    assert!(!map.view_state().center_held_by_terrain());
    map.map_context.style.terrain =
        Some(serde_json::from_str(r#"{"source":"dem"}"#).expect("terrain"));
    map.run_frame().expect("frame");
    assert!(
        map.view_state().center_held_by_terrain(),
        "terrain added at runtime holds it"
    );
    map.map_context.style.terrain = None;
    map.run_frame().expect("frame");
    assert!(!map.view_state().center_held_by_terrain());
    assert_eq!(map.view_state().center_elevation(), 3000.0);
}

#[test]
fn a_pose_set_before_the_first_frame_over_terrain_keeps_the_style_center_altitude() {
    use crate::{projection::ProjectionType, render::view_state::NavigationMode};
    let style = style(
        r#"{"version":8,"center":[88.05,27.77],"centerAltitude":3000,"zoom":11,"pitch":40,
            "sources":{"dem":{"type":"raster-dem","tiles":["https://dem.example/{z}/{x}/{y}.png"]}},
            "terrain":{"source":"dem"},"projection":{"type":"vertical-perspective"},"layers":[]}"#,
    );
    let mut view = initial_view_state(PhysicalSize::new(800, 600).expect("size"), &style);
    assert!(
        view.center_held_by_terrain(),
        "the style's terrain holds the center from the start"
    );
    view.set_navigation_mode(
        NavigationMode::FreeGlobe,
        &ProjectionType::VerticalPerspective,
    )
    .expect("free navigation");
    let mut pose = view.globe_pose().expect("pose");
    pose.target_elevation_meters = 500.0;
    view.set_globe_pose(pose).expect("pose");
    assert_eq!(view.center_altitude(), 3000.0);
    assert_eq!(view.center_elevation(), 500.0);
}

#![allow(clippy::expect_used, clippy::panic)]

use cgmath::{Deg, InnerSpace, Point2};

use super::{
    globe_pose, keep_globe_camera_above_terrain, look_from, recalculate_globe_zoom_and_center,
    zoom_globe_keeping_anchor, TerrainAnchor,
};
use crate::{
    coords::{LatLon, WorldCoords, Zoom, TILE_SIZE},
    render::{projection::globe_camera_for_view, view_state::ViewState},
    tcs::world::World,
    terrain::sightline::{
        lat_lon_to_mercator, pick_globe_terrain,
        synthetic::{hills, location, Ground, SCENE},
        TerrainPick,
    },
    window::PhysicalSize,
};

/// A center beside the antimeridian, where longitudes wrap between the eye and its target.
const ANTIMERIDIAN: LatLon = LatLon {
    latitude: -16.5,
    longitude: 179.995,
};
const BEARINGS: [f64; 4] = [30.0, 135.0, 210.0, 324.037_142_967_012_36];

/// The 2330x1800 Himalayan view orbiting the terrain at `elevation`.
fn view(zoom: f64, pitch: f64, bearing: f64, elevation: f64) -> ViewState {
    view_at(SCENE, zoom, pitch, bearing, elevation)
}

/// The 2330x1800 view of `center` orbiting the terrain at `elevation`.
fn view_at(center: LatLon, zoom: f64, pitch: f64, bearing: f64, elevation: f64) -> ViewState {
    let world_size = TILE_SIZE * 2_f64.powf(zoom);
    let mercator = lat_lon_to_mercator(center);
    let mut view = ViewState::new(
        PhysicalSize::new(2330, 1800).expect("viewport"),
        WorldCoords::from((mercator.x * world_size, mercator.y * world_size)),
        Zoom::new(zoom),
        Deg(0.0),
        Deg(36.869_897_645_844_02),
    );
    view.set_max_pitch(Deg(85.0));
    view.camera_mut().set_pitch(Deg(pitch));
    view.camera_mut().set_bearing(Deg(bearing));
    view.set_globe_orbits_terrain(true);
    view.set_center_elevation(elevation);
    view
}

fn world(ground: Ground) -> World {
    let mut world = World {
        tiles: ground.tiles,
        ..World::default()
    };
    world.resources.insert(ground.index);
    world
}

#[test]
fn the_pose_derived_from_an_eye_and_target_is_the_camera_s_own() {
    for (pitch, bearing) in [(0.0, 30.0), (70.0, 324.037_142_967_012_36), (85.0, 90.0)] {
        let mut state = view(11.67, pitch, bearing, 4000.0);
        let (eye, target) = globe_pose(&state).expect("orbiting camera");
        let before = (
            state.zoom().value(),
            state.camera().position(),
            state.camera().get_pitch().0,
            state.camera().get_bearing().0,
        );
        assert!(look_from(&mut state, eye, target));
        let (again, _) = globe_pose(&state).expect("orbiting camera");
        assert!((again - eye).magnitude() < 1e-12, "the eye moved");
        assert!((state.zoom().value() - before.0).abs() < 1e-9);
        assert!((state.camera().position() - before.1).magnitude() < 1e-6);
        assert!((state.camera().get_pitch().0 - before.2).abs() < 1e-9);
        if pitch > 0.0 {
            let turn =
                (state.camera().get_bearing().0 - before.3).rem_euclid(std::f64::consts::TAU);
            assert!(
                turn.min(std::f64::consts::TAU - turn) < 1e-9,
                "bearing {pitch}"
            );
        }
    }
}

#[test]
fn a_new_center_elevation_at_the_end_of_a_gesture_keeps_the_eye_in_place() {
    // Lower ground moves the target away and steepens the view at it; 80 degrees leaves room
    // under the pitch limit.
    let cases = [
        (0.0, 1500.0),
        (70.0, 1500.0),
        (85.0, 1500.0),
        (80.0, -300.0),
    ];
    for center in [SCENE, ANTIMERIDIAN] {
        for bearing in BEARINGS {
            for (pitch, elevation) in cases {
                let case = format!("{center:?}, bearing {bearing}, pitch {pitch}");
                let mut state = view_at(center, 12.0, pitch, bearing, 0.0);
                let (eye, target) = globe_pose(&state).expect("orbiting camera");
                assert!(recalculate_globe_zoom_and_center(&mut state, elevation));
                let (moved_eye, moved_target) = globe_pose(&state).expect("orbiting camera");
                assert!(
                    (moved_eye - eye).magnitude() < 1e-11,
                    "{case}: the eye moved by {} radii",
                    (moved_eye - eye).magnitude()
                );
                assert!(
                    ((moved_target - eye).normalize() - (target - eye).normalize()).magnitude()
                        < 1e-9,
                    "{case}: the view axis turned"
                );
                assert!((state.center_elevation() - elevation).abs() < 1e-6);
            }
        }
    }
}

#[test]
fn past_the_pitch_limit_the_eye_gives_and_the_target_still_lands_on_the_new_ground() {
    // At 85 degrees, lower ground would steepen the view at the target beyond the limit.
    let mut state = view(12.0, 85.0, 90.0, 0.0);
    let (eye, _) = globe_pose(&state).expect("orbiting camera");
    assert!(recalculate_globe_zoom_and_center(&mut state, -300.0));
    let (moved, target) = globe_pose(&state).expect("orbiting camera");
    assert!((state.camera().get_pitch().0 - 85_f64.to_radians()).abs() < 1e-12);
    assert!((state.center_elevation() + 300.0).abs() < 1e-6);
    assert!(((target.magnitude() - 1.0) * state.body().radius_meters + 300.0).abs() < 1e-3);
    assert!((moved - eye).magnitude() > 1e-9, "the eye gave way");
}

#[test]
fn an_eye_inside_the_ground_is_lifted_onto_the_ground_under_itself() {
    for center in [SCENE, ANTIMERIDIAN] {
        for bearing in [90.0, 200.0, 324.037_142_967_012_36] {
            // At zoom 14 and 85 degrees of pitch the eye flies a few hundred metres over its
            // target; the ground rises to 1500 m under the eye and stays at sea level under
            // the target.
            let mut state = view_at(center, 14.0, 85.0, bearing, 0.0);
            let (eye, target) = globe_pose(&state).expect("orbiting camera");
            let under = lat_lon_to_mercator(crate::projection::globe::unit_sphere_to_lat_lon(
                eye.normalize(),
            ));
            let reach = 3000.0 / state.body().circumference_meters();
            let world = world(Ground::around(center, move |mercator| {
                let dx = (mercator.x - under.x + 0.5).rem_euclid(1.0) - 0.5;
                if dx.hypot(mercator.y - under.y) < reach {
                    1500.0
                } else {
                    0.0
                }
            }));
            let case = format!("{center:?}, bearing {bearing}");
            let radius = state.body().radius_meters;
            assert!((eye.magnitude() - 1.0) * radius < 1500.0);
            assert!(
                keep_globe_camera_above_terrain(&mut state, &world),
                "{case}"
            );
            let (lifted, kept) = globe_pose(&state).expect("orbiting camera");
            let height = (lifted.magnitude() - 1.0) * radius;
            assert!(height >= 1500.0, "{case}: the eye is still {height} m high");
            assert!(
                (lifted.normalize() - eye.normalize()).magnitude() < 1e-12,
                "{case}"
            );
            assert!(
                (kept - target).magnitude() < 1e-11,
                "{case}: the target moved"
            );
            let camera = globe_camera_for_view(&state).expect("camera");
            let center_ray = camera
                .ray_direction_from_pixel(Point2::new(1165.0, 900.0))
                .expect("center ray");
            assert!(
                center_ray.dot((kept - lifted).normalize()) > 1.0 - 1e-12,
                "{case}"
            );
            assert!(
                !keep_globe_camera_above_terrain(&mut state, &world),
                "{case}"
            );
        }
    }
}

#[test]
fn a_picked_terrain_point_stays_under_the_pointer_through_a_zoom() {
    for center in [SCENE, ANTIMERIDIAN] {
        let ground = Ground::around(center, hills);
        let elevation = ground.terrain().ground_at(center).expect("ground");
        for (pitch, bearing, pointer) in [
            (70.0, 324.037_142_967_012_36, Point2::new(1700.0, 1200.0)),
            (85.0, 324.037_142_967_012_36, Point2::new(700.0, 1000.0)),
            (70.0, 135.0, Point2::new(500.0, 1300.0)),
        ] {
            let case = format!("{center:?}, pitch {pitch}, bearing {bearing}");
            let mut state = view_at(center, 10.8, pitch, bearing, elevation);
            state.freeze_center_elevation();
            let camera = globe_camera_for_view(&state).expect("camera");
            let TerrainPick::Ground(hit) = pick_globe_terrain(&camera, ground.terrain(), pointer)
            else {
                panic!("{case}: the pointer is over the ground");
            };
            let anchor = TerrainAnchor {
                location: location(hit.mercator),
                elevation: hit.elevation,
            };
            let mut zoom = 10.8;
            while zoom < 12.2 {
                zoom += 0.1;
                assert!(
                    zoom_globe_keeping_anchor(&mut state, anchor, pointer, zoom),
                    "{case}, zoom {zoom}: the anchor left the pointer"
                );
                let camera = globe_camera_for_view(&state).expect("camera");
                let point = crate::projection::globe::lat_lon_to_unit_sphere(anchor.location)
                    * camera.body().unit_radius_at(anchor.elevation);
                let clip = camera.view_projection() * point.extend(1.0);
                let at = camera.ndc_to_pixel(Point2::new(clip.x / clip.w, clip.y / clip.w));
                assert!(
                    (at - pointer).magnitude() < 1e-2,
                    "{case}, zoom {zoom}: the anchor shows at {at:?}, not {pointer:?}"
                );
                assert!((state.zoom().value() - zoom).abs() < 0.05, "zoom {zoom}");
            }
        }
    }
}

#[test]
fn a_host_s_eye_is_left_alone() {
    use cgmath::{Matrix4, Rad, SquareMatrix, Vector3};

    use crate::{
        projection::ProjectionType,
        render::{
            camera::EyeFrustum,
            view_state::{ExternalAnchor, ExternalView},
        },
    };
    let world = world(Ground::around(SCENE, |_| 1500.0));
    let mut state = view(12.0, 70.0, 0.0, 0.0);
    state
        .set_external_view(
            ExternalView {
                anchor: ExternalAnchor {
                    position: SCENE,
                    altitude_meters: 100.0,
                },
                view: (Matrix4::from_translation(Vector3::new(0.0, 0.0, 500.0))
                    * Matrix4::from_angle_x(Deg(70.0)))
                .invert()
                .expect("eye"),
                frustum: EyeFrustum::symmetric(Rad(1.2), 2330.0 / 1800.0, 0.1, 1.0e8),
            },
            &ProjectionType::VerticalPerspective,
        )
        .expect("eye");
    let before = globe_camera_for_view(&state)
        .expect("camera")
        .camera_position();
    assert_eq!(globe_pose(&state), None);
    assert!(!recalculate_globe_zoom_and_center(&mut state, 1000.0));
    assert!(!keep_globe_camera_above_terrain(&mut state, &world));
    assert_eq!(
        globe_camera_for_view(&state)
            .expect("camera")
            .camera_position(),
        before,
        "the eye stays where the host put it, inside the ground or not"
    );
}

#[test]
fn the_gesture_entry_points_take_the_globe_path_for_a_globe_that_orbits_the_terrain() {
    let style: crate::style::Style = serde_json::from_str(
        r#"{"version":8,"sources":{"dem":{"type":"raster-dem","tiles":["https://dem.example/{z}/{x}/{y}.png"],"encoding":"terrarium","tileSize":256}},"layers":[],"terrain":{"source":"dem"},"projection":{"type":"vertical-perspective"}}"#,
    )
    .expect("style");
    let world = world(Ground::around(SCENE, |_| 1500.0));
    // A gesture ends over ground higher than the elevation it held: the eye stays.
    let mut state = view(12.0, 70.0, 324.037_142_967_012_36, 0.0);
    state.freeze_center_elevation();
    let (eye, _) = globe_pose(&state).expect("orbiting camera");
    super::super::finish_gesture(&style, &mut state, &world);
    let (kept, _) = globe_pose(&state).expect("orbiting camera");
    assert!(
        (kept - eye).magnitude() < 1e-11,
        "the eye moved at the end of the gesture"
    );
    assert!((state.center_elevation() - 1500.0).abs() < 1.0);
    // An eye under the ground is lifted onto it.
    let mut state = view(14.0, 85.0, 90.0, 0.0);
    let (under, target) = globe_pose(&state).expect("orbiting camera");
    assert!(super::super::keep_camera_above_terrain(
        &style, &mut state, &world
    ));
    let (lifted, kept) = globe_pose(&state).expect("orbiting camera");
    assert!((lifted.magnitude() - 1.0) * state.body().radius_meters >= 1500.0);
    // Straight up from where it was, still looking at the same target.
    assert!((lifted.normalize() - under.normalize()).magnitude() < 1e-12);
    assert!((kept - target).magnitude() < 1e-11);
}

#[test]
fn an_anchor_that_cannot_reach_its_pixel_leaves_the_view_as_it_was() {
    let mut state = view(12.0, 85.0, 90.0, 0.0);
    let (zoom, position) = (state.zoom().value(), state.camera().position());
    let anchor = TerrainAnchor {
        location: SCENE,
        elevation: 0.0,
    };
    // The top of the screen shows the sky, which no sphere near the ground reaches.
    assert!(!super::place_anchor_at_pixel(
        &mut state,
        anchor,
        Point2::new(1165.0, 1.0)
    ));
    assert_eq!(state.zoom().value(), zoom);
    assert_eq!(state.camera().position(), position);
}

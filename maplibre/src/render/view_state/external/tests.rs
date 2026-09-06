#![allow(clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;

use cgmath::{Deg, InnerSpace, Matrix4, Rad, SquareMatrix, Vector3};

use super::{ExternalAnchor, ExternalView, ExternalViewError};
use crate::{
    coords::{LatLon, WorldCoords, WorldTileCoords, Zoom, ZoomLevel},
    projection::ProjectionType,
    render::{
        camera::{EdgeInsets, EyeFrustum, FLIP_Y, OPENGL_TO_WGPU_MATRIX},
        projection::{globe_camera_for_view, mercator_world_to_lat_lon},
        view_state::{CameraPose, ViewState, ViewStatePadding},
    },
    window::PhysicalSize,
};

const FOVY: Rad<f64> = Rad(0.6435011087932844);
const MERCATOR: ProjectionType = ProjectionType::Mercator;
const GLOBE: ProjectionType = ProjectionType::Globe;

fn view_state(zoom: f64, position: LatLon) -> ViewState {
    let zoom = Zoom::new(zoom);
    let mut view_state = ViewState::new(
        PhysicalSize::new(800, 600).expect("non-zero size"),
        WorldCoords::from_lat_lon(position, zoom),
        zoom,
        Deg(0.0),
        FOVY,
    );
    view_state.set_max_pitch(Deg(180.0));
    view_state
}

fn posed_view_state() -> ViewState {
    let mut view_state = view_state(10.0, LatLon::new(47.0, 11.0));
    view_state.set_center_elevation(650.0);
    view_state.set_camera_pose(CameraPose {
        position: LatLon::new(47.3, 11.4),
        altitude_meters: 3000.0,
        bearing: Deg(35.0),
        pitch: Deg(55.0),
        roll: Deg(12.0),
    });
    view_state
}

fn assert_matrices_close(a: Matrix4<f64>, b: Matrix4<f64>, what: &str) {
    for column in 0..4 {
        for row in 0..4 {
            let (x, y) = (a[column][row], b[column][row]);
            assert!(
                (x - y).abs() <= 1e-9 * x.abs().max(y.abs()).max(1.0),
                "{what} differs at column {column} row {row}: {x} vs {y}"
            );
        }
    }
}

fn assert_poses_close(a: CameraPose, b: CameraPose) {
    assert!(
        (a.position.latitude - b.position.latitude).abs() < 1e-9,
        "{a:?} vs {b:?}"
    );
    assert!(
        (a.position.longitude - b.position.longitude).abs() < 1e-9,
        "{a:?} vs {b:?}"
    );
    assert!(
        (a.altitude_meters - b.altitude_meters).abs() < 1e-6,
        "{a:?} vs {b:?}"
    );
    assert!((a.bearing.0 - b.bearing.0).abs() < 1e-9, "{a:?} vs {b:?}");
    assert!((a.pitch.0 - b.pitch.0).abs() < 1e-9, "{a:?} vs {b:?}");
    assert!((a.roll.0 - b.roll.0).abs() < 1e-9, "{a:?} vs {b:?}");
}

fn covering(view_state: &ViewState) -> BTreeSet<WorldTileCoords> {
    view_state
        .create_view_region(ZoomLevel::new(12), ViewStatePadding::Tight)
        .expect("a view region")
        .iter()
        .collect()
}

#[test]
fn injecting_the_maps_own_matrices_reproduces_its_frame_and_covering() {
    let original = posed_view_state();
    let mut injected = view_state(4.0, LatLon::new(0.0, 0.0));
    injected.set_center_elevation(650.0);

    injected
        .set_external_view(original.external_view(), &MERCATOR)
        .expect("the map's own view is invertible");

    assert_poses_close(original.camera_pose(), injected.camera_pose());
    assert!((original.zoom().value() - injected.zoom().value()).abs() < 1e-9);
    assert_matrices_close(
        original.view_projection().0,
        injected.view_projection().0,
        "view projection",
    );
    assert_eq!(covering(&original), covering(&injected));
}

#[test]
fn an_off_center_perspective_survives_the_round_trip() {
    let mut original = posed_view_state();
    original.set_edge_insets(EdgeInsets {
        top: 120.0,
        bottom: 0.0,
        left: 40.0,
        right: 0.0,
    });
    let mut injected = view_state(4.0, LatLon::new(0.0, 0.0));
    injected.set_center_elevation(650.0);

    injected
        .set_external_view(original.external_view(), &MERCATOR)
        .expect("the map's own view is invertible");

    assert_matrices_close(
        original.view_projection().0,
        injected.view_projection().0,
        "view projection",
    );
}

#[test]
fn a_per_eye_frustum_is_used_as_it_is_and_moves_the_covering() {
    let original = posed_view_state();
    let symmetric = original.external_view();
    // A frustum shifted far to one side, as an eye of a stereo pair would be.
    let eye = ExternalView {
        frustum: EyeFrustum {
            left: symmetric.frustum.left * 0.2,
            right: symmetric.frustum.right * 1.8,
            ..symmetric.frustum
        },
        ..symmetric
    };
    let mut injected = view_state(4.0, LatLon::new(0.0, 0.0));
    injected.set_center_elevation(650.0);

    injected
        .set_external_view(eye, &MERCATOR)
        .expect("invertible");

    // The frustum the map hands out is in its camera pixels, which is what it renders with.
    let projection = eye.frustum.projection();
    let expected = OPENGL_TO_WGPU_MATRIX * projection * FLIP_Y * {
        // The camera matrix is what the map's own perspective would be applied to.
        let mut own = injected.clone();
        own.clear_external_view();
        let own_projection = symmetric.frustum.projection();
        (FLIP_Y * OPENGL_TO_WGPU_MATRIX * (FLIP_Y * own_projection * FLIP_Y))
            .invert()
            .expect("invertible")
            * own.view_projection().0
    };
    assert_matrices_close(
        injected.view_projection().0,
        expected,
        "sheared view projection",
    );
    assert_ne!(covering(&original), covering(&injected));
    assert_eq!(injected.external_projection(), Some(projection));

    injected.clear_external_view();
    assert_eq!(injected.external_projection(), None);
    assert_matrices_close(
        original.view_projection().0,
        injected.view_projection().0,
        "the map's own perspective returns",
    );
}

#[test]
fn a_view_in_other_units_still_measures_the_pose_in_metres() {
    let original = posed_view_state();
    let own = original.external_view();
    // A host showing the map at a thousandth of its size renders in units a thousand times
    // smaller than the local metres, clip distances included.
    let scale = 1e-3;
    let model = ExternalView {
        view: Matrix4::from_scale(scale) * own.view,
        frustum: own.frustum.scaled(scale),
        ..own
    };
    let mut injected = view_state(4.0, LatLon::new(0.0, 0.0));
    injected.set_center_elevation(650.0);

    injected
        .set_external_view(model, &MERCATOR)
        .expect("a scaled view is invertible");

    assert_poses_close(original.camera_pose(), injected.camera_pose());
    assert_matrices_close(
        original.view_projection().0,
        injected.view_projection().0,
        "view projection",
    );
}

#[test]
fn a_view_pitched_beyond_the_limit_still_draws_what_the_eye_sees() {
    let mut original = posed_view_state();
    // Without roll, turning about the eye's right axis stays in the vertical plane.
    original.set_camera_pose(CameraPose {
        roll: Deg(0.0),
        ..original.camera_pose()
    });
    let own = original.external_view();
    // The eye turns to look 30 degrees above level: pitch 120, past any limit the map has.
    let looking_up = ExternalView {
        view: Matrix4::from_angle_x(Deg(-65.0)) * own.view,
        ..own
    };
    let mut limited = view_state(4.0, LatLon::new(0.0, 0.0));
    limited.set_max_pitch(Deg(60.0));

    limited
        .set_external_view(looking_up, &MERCATOR)
        .expect("a view looking up is not refused");

    let pitch: Deg<f64> = limited.camera().get_pitch().into();
    assert!(
        pitch.0 <= 60.0 + 1e-9,
        "the pose keeps to the limit: {pitch:?}"
    );
    // The bookkeeping zoom follows the distance to a point near the horizon, not a point
    // beyond the sky, so a glance upwards does not request tiles at a wildly different zoom.
    let zoom_shift = (limited.zoom().value() - original.zoom().value()).abs();
    assert!(
        zoom_shift < 2.0,
        "looking up keeps the zoom near the level gaze's: shifted by {zoom_shift}"
    );
    // The frame comes from the eye: the eye sits where the matrix puts it, looking the way it
    // looks, whatever the clamped pose says.
    let located = |view_state: &ViewState| {
        let eye = view_state.eye_position();
        let world_size = crate::coords::TILE_SIZE * 2f64.powf(view_state.zoom().value());
        (mercator_world_to_lat_lon(eye.x, eye.y, world_size), eye.z)
    };
    let (eye_location, eye_altitude) = located(&limited);
    let (expected_location, expected_altitude) = located(&original);
    assert!(
        (eye_location.latitude - expected_location.latitude).abs() < 1e-9
            && (eye_location.longitude - expected_location.longitude).abs() < 1e-9
            && (eye_altitude - expected_altitude).abs() < 1e-6,
        "{eye_location:?} at {eye_altitude} vs {expected_location:?} at {expected_altitude}"
    );
    let forward_of = |view_state: &ViewState| {
        let inverse = view_state
            .view_projection()
            .0
            .invert()
            .expect("an eye view projection is invertible");
        let near = inverse * cgmath::Vector4::new(0.0, 0.0, 0.0, 1.0);
        let far = inverse * cgmath::Vector4::new(0.0, 0.0, 1.0, 1.0);
        let direction = far.truncate() / far.w - near.truncate() / near.w;
        // World x and y are pixels, z metres: bring z to pixels before normalizing.
        let pixels_per_meter = view_state.pixels_per_meter();
        Vector3::new(direction.x, direction.y, direction.z * pixels_per_meter).normalize()
    };
    let level = forward_of(&original);
    assert!(
        (level.z + 55f64.to_radians().cos()).abs() < 1e-6,
        "the fixture looks 55 degrees from straight down: {level:?}"
    );
    let forward = forward_of(&limited);
    // World z is up; 30 degrees above level means a positive upward component of one half.
    // The pixels per metre the map reports are those of its clamped center, a little away
    // from the anchor the eye is measured from, which is where the last digits go.
    assert!((forward.z - 0.5).abs() < 1e-3, "{forward:?}");
}

#[test]
fn an_overscanned_view_widens_the_eye_for_requests_only() {
    let original = posed_view_state();
    let mut injected = view_state(4.0, LatLon::new(0.0, 0.0));
    injected
        .set_external_view(original.external_view(), &MERCATOR)
        .expect("invertible");
    assert!(injected.overscanned(1.0).is_none());
    assert!(
        original.overscanned(1.5).is_none(),
        "no eye, nothing to widen"
    );

    let widened = injected.overscanned(1.5).expect("an eye to widen");

    let (narrow, wide) = (
        injected.external_projection().expect("projection"),
        widened.external_projection().expect("projection"),
    );
    assert!((wide.x.x * 1.5 - narrow.x.x).abs() < 1e-9, "{wide:?}");
    assert!((wide.y.y * 1.5 - narrow.y.y).abs() < 1e-9, "{wide:?}");
    assert_eq!(wide.w.z, narrow.w.z, "clip distances stay");
    assert!(covering(&widened).len() > covering(&injected).len());
    assert_eq!(
        injected.external_projection(),
        Some(narrow),
        "the frame is untouched"
    );
}

#[test]
fn a_frustum_without_volume_is_refused() {
    let mut view_state = posed_view_state();
    let own = view_state.external_view();
    let before = view_state.view_projection().0;
    for frustum in [
        EyeFrustum {
            far: f64::INFINITY,
            ..own.frustum
        },
        EyeFrustum {
            near: 0.0,
            ..own.frustum
        },
        EyeFrustum {
            far: own.frustum.near,
            ..own.frustum
        },
        EyeFrustum {
            left: -own.frustum.right,
            ..own.frustum
        },
        EyeFrustum {
            top: f64::NAN,
            ..own.frustum
        },
    ] {
        let refused = view_state.set_external_view(ExternalView { frustum, ..own }, &MERCATOR);
        assert!(
            matches!(refused, Err(ExternalViewError::InvalidFrustum { .. })),
            "{frustum:?} gave {refused:?}"
        );
    }
    assert_eq!(view_state.view_projection().0, before);
    assert_eq!(view_state.external_projection(), None);
}

#[test]
fn a_singular_view_matrix_is_refused() {
    let mut view_state = posed_view_state();
    let external = ExternalView {
        view: Matrix4::from_scale(0.0),
        ..view_state.external_view()
    };
    assert_eq!(
        view_state.set_external_view(external, &MERCATOR),
        Err(ExternalViewError::SingularView)
    );
    assert_eq!(view_state.external_projection(), None);
    assert!(view_state.external_globe_eye().is_none());
}

/// A view state far enough out for the globe to be drawn, looking down at an angle.
fn globe_view_state() -> ViewState {
    let mut view_state = view_state(3.0, LatLon::new(30.0, -20.0));
    view_state.set_camera_pose(CameraPose {
        position: LatLon::new(30.0, -20.0),
        altitude_meters: 3_000_000.0,
        bearing: Deg(20.0),
        pitch: Deg(30.0),
        roll: Deg(0.0),
    });
    view_state
}

#[test]
fn on_the_globe_the_eye_becomes_the_globe_camera() {
    let original = globe_view_state();
    let mut injected = view_state(4.0, LatLon::new(0.0, 0.0));

    injected
        .set_external_view(original.external_view(), &GLOBE)
        .expect("the map's own view is invertible");

    assert!(
        (original.zoom().value() - injected.zoom().value()).abs() < 1e-9,
        "{} vs {}",
        original.zoom().value(),
        injected.zoom().value()
    );
    let native = globe_camera_for_view(&original).expect("a globe camera");
    let external = globe_camera_for_view(&injected).expect("a globe camera from the eye");
    assert_matrices_close(external.view(), native.view(), "globe view");
    let (a, b) = (external.camera_position(), native.camera_position());
    assert!((a - b).magnitude() < 1e-9, "{a:?} vs {b:?}");
    let (a, b) = (external.clipping_plane(), native.clipping_plane());
    assert!((a - b).magnitude() < 1e-9, "{a:?} vs {b:?}");
    // The frustum's angles are the map's own; only its clip distances differ, since the
    // map's globe camera picks its own.
    let (a, b) = (external.projection(), native.projection());
    for (x, y) in [
        (a.x.x, b.x.x),
        (a.y.y, b.y.y),
        (a.z.x, b.z.x),
        (a.z.y, b.z.y),
    ] {
        assert!((x - y).abs() < 1e-9, "{x} vs {y}");
    }
}

#[test]
fn a_model_globe_places_the_eye_above_the_anchor_at_the_model_scale() {
    let mut injected = view_state(4.0, LatLon::new(0.0, 0.0));
    let radius = injected.body().radius_meters;
    // A globe the size of a football, seen from two radii above a point on its equator.
    let scale = 0.11 / radius;
    let eye_height = 2.0 * radius;
    let model = ExternalView {
        anchor: ExternalAnchor {
            position: LatLon::new(0.0, 0.0),
            altitude_meters: 0.0,
        },
        view: Matrix4::from_scale(scale)
            * Matrix4::look_at_rh(
                cgmath::Point3::new(0.0, 0.0, eye_height),
                cgmath::Point3::new(0.0, 0.0, 0.0),
                Vector3::unit_y(),
            ),
        frustum: EyeFrustum::symmetric(Rad(1.2), 1.5, 0.05, 20.0),
    };

    injected
        .set_external_view(model, &GLOBE)
        .expect("the eye sits above the globe");

    let eye = injected
        .external_globe_eye()
        .expect("an external eye is in effect");
    assert!(
        (eye.position - Vector3::new(0.0, 0.0, 3.0)).magnitude() < 1e-9,
        "{:?}",
        eye.position
    );
    let pose = injected.camera_pose();
    assert!((pose.altitude_meters - eye_height).abs() < 1e-3, "{pose:?}");
    assert!(pose.pitch.0.abs() < 1e-9, "{pose:?}");
    let pixels_per_meter = injected.pixels_per_meter();
    assert!(
        (eye.frustum.near - 0.05 / scale * pixels_per_meter).abs() < 1e-9,
        "near {} in pixels",
        eye.frustum.near
    );
    assert!(
        (eye.camera_to_center_distance - eye_height * pixels_per_meter).abs() < 1e-6,
        "distance {} in pixels",
        eye.camera_to_center_distance
    );
    let camera = globe_camera_for_view(&injected).expect("a globe camera from the eye");
    assert!(
        (camera.clipping_plane() - cgmath::Vector4::new(0.0, 0.0, 1.0, -1.0 / 3.0)).magnitude()
            < 1e-9
    );
}

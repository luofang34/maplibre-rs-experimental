#![allow(clippy::expect_used, clippy::panic)]
use super::*;
use crate::{
    coords::{LatLon, WorldCoords, Zoom},
    render::{camera::EyeFrustum, view_state::ExternalAnchor},
    window::PhysicalSize,
};
use cgmath::{Deg, Rad};

fn view(x: f64, turn: f64) -> ExternalView {
    ExternalView {
        anchor: ExternalAnchor {
            position: LatLon::new(47.26, 11.39),
            altitude_meters: 600.0,
        },
        view: (Matrix4::from_translation(Vector3::new(x, 0.0, 4000.0))
            * Matrix4::from_angle_y(Deg(turn)))
        .invert()
        .expect("pose"),
        frustum: EyeFrustum::symmetric(Rad(1.2), 1.0, 0.05, 1e8),
    }
}
fn base() -> ViewState {
    ViewState::new(
        PhysicalSize::new(2048, 2048).expect("size"),
        WorldCoords::from((256.0, 256.0)),
        Zoom::new(10.0),
        Deg(0.0),
        Deg(45.0),
    )
}
#[test]
fn movement_and_turn_predict_a_bounded_future_view() {
    let previous = view(0.0, 0.0);
    let current = view(100.0, 10.0);
    let ahead =
        predict(previous, current, 0.1, &base(), &ProjectionType::Mercator).expect("prediction");
    let mut now = base();
    now.set_external_view(current, &ProjectionType::Mercator)
        .expect("view");
    assert!(ahead.camera_pose().position.longitude > now.camera_pose().position.longitude);
    assert!((ahead.camera_pose().pitch.0 - now.camera_pose().pitch.0).abs() <= 20.01);
    assert!(ahead.frustum_corners().is_ok());
    assert!(predict(current, current, 0.1, &base(), &ProjectionType::Mercator).is_none());
}
#[test]
fn sampling_holds_predictions_and_resets_after_tracking_gaps() {
    let mut motion = MotionPrefetch::default();
    assert!(motion
        .update(
            Duration::ZERO,
            view(0.0, 0.0),
            &base(),
            &ProjectionType::Mercator
        )
        .is_none());
    let a = motion
        .update(
            Duration::from_millis(100),
            view(50.0, 2.0),
            &base(),
            &ProjectionType::Mercator,
        )
        .expect("moving");
    let b = motion
        .update(
            Duration::from_millis(105),
            view(51.0, 2.1),
            &base(),
            &ProjectionType::Mercator,
        )
        .expect("held");
    assert_eq!(a.view_projection().0, b.view_projection().0);
    assert!(motion
        .update(
            Duration::from_secs(2),
            view(1000.0, 40.0),
            &base(),
            &ProjectionType::Mercator
        )
        .is_none());
}
#[test]
fn anchor_rebasing_does_not_predict_false_camera_motion() {
    let previous = view(0.0, 0.0);
    let mut current = previous;
    current.anchor.altitude_meters += 100.0;
    current.view = Matrix4::from_translation(Vector3::new(0.0, 0.0, -3900.0));
    assert!(predict(previous, current, 0.1, &base(), &ProjectionType::Mercator).is_none());
}

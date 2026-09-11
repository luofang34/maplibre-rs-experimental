#![allow(clippy::expect_used, clippy::panic)]
use super::*;
use crate::{
    coords::{WorldCoords, Zoom},
    window::PhysicalSize,
};
use cgmath::{Deg, SquareMatrix};

#[test]
fn terrain_arrivals_do_not_move_a_stationary_eyes_lod_center() {
    let mut view = ViewState::new(
        PhysicalSize::new(1024, 1024).expect("viewport"),
        WorldCoords::from((256.0, 256.0)),
        Zoom::new(10.0),
        Deg(0.0),
        Deg(45.0),
    );
    let external = ExternalView {
        anchor: ExternalAnchor {
            position: LatLon::new(47.26, 11.39),
            altitude_meters: 600.0,
        },
        view: (Matrix4::from_translation(Vector3::new(0.0, 0.0, 4000.0))
            * Matrix4::from_angle_x(Deg(90.0)))
        .invert()
        .expect("eye"),
        frustum: EyeFrustum::symmetric(Rad(1.4), 1.0, 0.05, 1.0e8),
    };
    view.set_external_view(external, &ProjectionType::Mercator)
        .expect("pose");
    let zoom = view.zoom();
    let center = view.camera().position();
    for elevation in [2400.0, 800.0, 1600.0] {
        view.set_center_elevation(elevation);
        view.set_external_view(external, &ProjectionType::Mercator)
            .expect("same pose");
        assert_eq!(view.zoom().value(), zoom.value());
        assert_eq!(view.camera().position(), center);
        assert!(view.eye_settled(), "terrain data is not head motion");
    }
}

#[test]
fn immersive_gaze_crossing_the_horizon_does_not_switch_projection() {
    for height in [150.0, 4000.0] {
        let mut view = ViewState::new(
            PhysicalSize::new(1888, 1792).expect("viewport"),
            WorldCoords::from((256.0, 256.0)),
            Zoom::new(14.0),
            Deg(0.0),
            Deg(36.87),
        );
        let mut previous: Option<f64> = None;
        for step in 0..=80 {
            let pitch = 86.0 + f64::from(step) * 0.1;
            let external = ExternalView {
                anchor: ExternalAnchor {
                    position: LatLon::new(47.26, 11.39),
                    altitude_meters: 600.0,
                },
                view: (Matrix4::from_translation(Vector3::new(0.0, 0.0, height))
                    * Matrix4::from_angle_x(Deg(pitch)))
                .invert()
                .expect("eye"),
                frustum: EyeFrustum::symmetric(Rad(1.4), 1888.0 / 1792.0, 0.05, 1.0e8),
            };
            view.set_external_view(external, &ProjectionType::Globe)
                .expect("pose");
            let zoom = view.zoom().value();
            assert!(
                !ProjectionType::Globe.uses_globe_rendering(zoom),
                "a nearby eye must stay flat at pitch {pitch}, height {height}, zoom {zoom}"
            );
            if let Some(previous) = previous {
                assert!(
                    (zoom - previous).abs() < 0.3,
                    "horizon crossing jumped from {previous} to {zoom} at pitch {pitch}"
                );
            }
            previous = Some(zoom);
        }
    }
}

#[test]
fn head_rotation_preserves_cartographic_scale_but_translation_changes_it() {
    for projection in [ProjectionType::Mercator, ProjectionType::Globe] {
        let mut view = ViewState::new(
            PhysicalSize::new(1024, 1024).expect("viewport"),
            WorldCoords::from((256.0, 256.0)),
            Zoom::new(10.0),
            Deg(0.0),
            Deg(45.0),
        );
        let external = |pitch, height| ExternalView {
            anchor: ExternalAnchor {
                position: LatLon::new(47.26, 11.39),
                altitude_meters: 600.0,
            },
            view: (Matrix4::from_translation(Vector3::new(0.0, 0.0, height))
                * Matrix4::from_angle_x(Deg(pitch)))
            .invert()
            .expect("eye"),
            frustum: EyeFrustum::symmetric(Rad(1.4), 1.0, 0.05, 1e8),
        };
        view.set_external_view(external(0.0, 2000.0), &projection)
            .expect("eye");
        let zoom = view.style_zoom().value();
        for pitch in [30.0, 60.0, 80.0, 90.0, 110.0, 170.0] {
            view.set_external_view(external(pitch, 2000.0), &projection)
                .expect("turned eye");
            assert!(
                (view.style_zoom().value() - zoom).abs() < 1e-8,
                "head pitch {pitch} changed style zoom"
            );
            assert!(
                view.eye_settled(),
                "looking around cannot suppress refinement"
            );
        }
        view.set_external_view(external(60.0, 4000.0), &projection)
            .expect("moved eye");
        assert!(
            view.style_zoom().value() < zoom - 0.7,
            "physical movement must still change scale"
        );
    }
}

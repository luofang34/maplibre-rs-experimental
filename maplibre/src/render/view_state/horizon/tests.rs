#![allow(clippy::expect_used, clippy::panic)]
use super::*;
use crate::{
    coords::{LatLon, WorldCoords, Zoom},
    projection::ProjectionType,
    render::{
        camera::EyeFrustum,
        view_state::{ExternalAnchor, ExternalView},
    },
    window::PhysicalSize,
};
use cgmath::SquareMatrix;
use cgmath::{Deg, Matrix4, Rad, Vector3};

#[test]
fn sky_side_is_continuous_when_gaze_and_ground_center_cross_the_horizon() {
    let mut view = ViewState::new(
        PhysicalSize::new(1888, 1792).expect("viewport"),
        WorldCoords::from((256.0, 256.0)),
        Zoom::new(12.0),
        Deg(0.0),
        Deg(60.0),
    );
    for roll in [-75.0_f64, 0.0, 45.0, 150.0] {
        let mut previous: Option<HorizonLine> = None;
        for step in 0..=160 {
            let pitch = 50.0 + f64::from(step) * 0.5;
            let external = ExternalView {
                anchor: ExternalAnchor {
                    position: LatLon::new(47.26, 11.39),
                    altitude_meters: 600.0,
                },
                view: (Matrix4::from_translation(Vector3::new(0.0, 0.0, 4000.0))
                    * Matrix4::from_angle_x(Deg(pitch))
                    * Matrix4::from_angle_z(Deg(roll)))
                .invert()
                .expect("eye"),
                frustum: EyeFrustum::symmetric(Rad(1.4), 1888.0 / 1792.0, 0.05, 1.0e8),
            };
            view.set_external_view(external, &ProjectionType::Mercator)
                .expect("pose");
            let line = view.horizon_line();
            let expected = Vector2::new(roll.to_radians().sin(), roll.to_radians().cos());
            assert!(
                (line.normal - expected).magnitude() < 1e-7,
                "pitch {pitch}, roll {roll}: {line:?}"
            );
            if let Some(previous) = previous {
                let center = Point2::new(944.0, 896.0);
                assert!((line.sky_distance(center) - previous.sky_distance(center)).abs() < 20.0);
            }
            previous = Some(line);
        }
    }
}

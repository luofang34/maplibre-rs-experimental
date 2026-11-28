#![allow(clippy::expect_used, clippy::panic)]
use cgmath::{Deg, Matrix4, Rad, SquareMatrix, Vector3};

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

mod view_state_tests {
    use cgmath::{Deg, Matrix4, Vector2, Vector4};

    use crate::{
        coords::{WorldCoords, Zoom},
        render::view_state::ViewState,
        window::PhysicalSize,
    };

    #[test]
    fn conform_transformation() {
        let fov = Deg(60.0);
        let mut state = ViewState::new(
            PhysicalSize::new(800, 600).expect("valid test view"),
            WorldCoords::at_ground(0.0, 0.0),
            Zoom::new(10.0),
            Deg(0.0),
            fov,
        );

        let projection = state.inverted_view_projection().expect("valid view");

        let bottom_left = state
            .window_to_world_at_ground(&Vector2::new(0.0, 0.0), &projection, true)
            .expect("valid test view");
        tracing::debug!("bottom left on ground {:?}", bottom_left);
        let top_right = state
            .window_to_world_at_ground(
                &Vector2::new(state.width(), state.height()),
                &projection,
                true,
            )
            .expect("valid test view");
        tracing::debug!("top right on ground {:?}", top_right);

        let mut rotated = Matrix4::from_angle_x(Deg(-30.0))
            * Vector4::new(bottom_left.x, bottom_left.y, 0.0, 0.0);

        tracing::debug!("bottom left rotated around x axis {:?}", rotated);

        rotated = Matrix4::from_angle_y(Deg(-30.0)) * rotated;

        tracing::debug!("bottom left rotated around x and y axis {:?}", rotated);

        state.camera_mut().set_pitch(Deg(30.0));
    }

    fn state_at(zoom: f64, pitch: Deg<f64>) -> ViewState {
        ViewState::new(
            PhysicalSize::new(800, 600).expect("valid test view"),
            WorldCoords::at_ground(256.0 * 2.0_f64.powf(zoom), 256.0 * 2.0_f64.powf(zoom)),
            Zoom::new(zoom),
            pitch,
            Deg(36.87),
        )
    }

    #[test]
    fn pixels_per_meter_follows_world_size_at_the_equator() {
        let state = state_at(0.0, Deg(0.0));
        let expected = 512.0 / (2.0 * std::f64::consts::PI * 6_371_008.8);

        assert!((state.pixels_per_meter() - expected).abs() < 1e-12);
        assert!((state_at(3.0, Deg(0.0)).pixels_per_meter() - expected * 8.0).abs() < 1e-12);
    }

    #[test]
    fn center_elevation_keeps_the_elevated_center_on_screen_center() {
        let mut state = state_at(12.0, Deg(45.0));
        state.set_center_elevation(570.0);
        let center = state.camera().position();

        let clip = state
            .view_projection()
            .project(Vector4::new(center.x, center.y, 570.0, 1.0));
        let window = state.clip_to_window(&clip);

        assert!((window.x - 400.0).abs() < 1e-6);
        assert!((window.y - 300.0).abs() < 1e-6);
    }

    #[test]
    fn a_bearing_of_ninety_degrees_puts_east_at_the_top_of_the_screen() {
        let mut state = state_at(2.0, Deg(0.0));
        state.camera_mut().set_bearing(Deg(90.0));
        let center = state.camera().position();

        let clip =
            state
                .view_projection()
                .project(Vector4::new(center.x + 100.0, center.y, 0.0, 1.0));
        let window = state.clip_to_window(&clip);

        assert!(
            (window.x - 400.0).abs() < 1e-6,
            "east stays centred: {window:?}"
        );
        assert!(window.y < 300.0, "east is above the centre: {window:?}");
    }

    #[test]
    fn far_plane_stays_finite_at_high_pitch() {
        let mut state = state_at(12.0, Deg(0.0));
        state.set_max_pitch(Deg(85.0));
        state.camera_mut().set_pitch(Deg(85.0));
        let (near, far) = state.depth_range(cgmath::Point2::new(0.0, 0.0));

        assert!(near > 0.0);
        assert!(far.is_finite());
        assert!(far > state.camera_to_center_distance());
        assert!((state.camera().get_pitch().0 - 85.0_f64.to_radians()).abs() < 1e-9);
    }

    #[test]
    fn far_plane_matches_the_camera_distance_when_flat() {
        let state = state_at(5.0, Deg(0.0));
        let (_, far) = state.depth_range(cgmath::Point2::new(0.0, 0.0));

        assert!((far - state.camera_to_center_distance() * 1.01).abs() < 1e-6);
    }

    #[test]
    fn max_pitch_clamps_pitch_changes() {
        let mut state = state_at(5.0, Deg(70.0));
        assert!((state.camera().get_pitch().0 - 60.0_f64.to_radians()).abs() < 1e-9);

        state.set_max_pitch(Deg(85.0));
        state.camera_mut().set_pitch(Deg(90.0));
        assert!((state.camera().get_pitch().0 - 85.0_f64.to_radians()).abs() < 1e-9);
    }

    #[test]
    fn gpu_view_projection_reverses_depth_only() {
        let state = ViewState::new(
            PhysicalSize::new(800, 600).expect("valid test view"),
            WorldCoords::at_ground(1024.0, 2048.0),
            Zoom::new(10.0),
            Deg(25.0),
            Deg(60.0),
        );
        let point = Vector4::new(1000.0, 2100.0, 0.0, 1.0);

        let cpu = state.view_projection().project(point);
        let gpu = state.gpu_view_projection().project(point);

        assert!((cpu.x - gpu.x).abs() < 1e-9);
        assert!((cpu.y - gpu.y).abs() < 1e-9);
        assert!((cpu.w - gpu.w).abs() < 1e-9);
        assert!((gpu.z / gpu.w - (1.0 - cpu.z / cpu.w)).abs() < 1e-9);
    }
}

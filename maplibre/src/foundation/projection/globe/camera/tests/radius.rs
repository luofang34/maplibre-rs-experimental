//! A camera given its globe's radius directly is the camera that derives it from the world
//! size wherever the world reaches, and stays finite over the poles where it does not.

use cgmath::{Point2, SquareMatrix};

use super::{options, GlobeCameraOptions, GlobeCameraState};
use crate::{coords::LatLon, projection::globe::scale};

fn view(latitude: f64, zoom: f64, radius_pixels: Option<f64>) -> GlobeCameraOptions {
    GlobeCameraOptions {
        width: 1200.0,
        height: 900.0,
        center: LatLon::new(latitude, 30.0),
        world_size: 512.0 * 2_f64.powf(zoom),
        bearing_degrees: 40.0,
        pitch_degrees: 60.0,
        radius_pixels,
        ..options()
    }
}

#[test]
fn a_given_radius_inside_the_world_is_the_derived_one() {
    for latitude in [-85.0, -30.0, 0.0, 60.0, 85.05] {
        for zoom in [1.0, 8.0, 15.0] {
            let derived = GlobeCameraState::new(view(latitude, zoom, None)).expect("camera");
            let given = GlobeCameraState::new(view(
                latitude,
                zoom,
                Some(scale::radius_pixels(zoom, latitude)),
            ))
            .expect("camera");
            let difference = given.view_projection() - derived.view_projection();
            let columns: [[f64; 4]; 4] = difference.into();
            let scale_of = derived.view_projection().determinant().abs().powf(0.25);
            assert!(
                columns
                    .iter()
                    .flatten()
                    .all(|value| value.abs() <= 1e-9 * scale_of),
                "{latitude}, {zoom}: the matrices differ"
            );
            assert!((given.pixel_scale() / derived.pixel_scale() - 1.0).abs() < 1e-12);
            assert!(
                (given.circle_radius_correction() / derived.circle_radius_correction() - 1.0).abs()
                    < 1e-12
            );
        }
    }
}

#[test]
fn a_given_radius_keeps_the_camera_finite_up_to_the_pole() {
    for latitude in [85.5, 87.0, 89.0, 89.999_999, 90.0, -90.0] {
        let radius = scale::radius_pixels(6.0, latitude);
        let zoom = scale::style_zoom(radius, latitude);
        let camera =
            GlobeCameraState::new(view(latitude, zoom, Some(radius))).expect("camera at the pole");
        let columns: [[f64; 4]; 4] = camera.view_projection().into();
        assert!(
            columns.iter().flatten().all(|value| value.is_finite()),
            "{latitude}"
        );
        assert!(camera.pixel_scale().is_finite() && camera.pixel_scale() > 0.0);
        let center = camera.location_to_screen(camera.center(), 0.0);
        assert!(
            (center - Point2::new(600.0, 450.0)).x.abs() < 1e-6,
            "{latitude}: {center:?}"
        );
    }
}

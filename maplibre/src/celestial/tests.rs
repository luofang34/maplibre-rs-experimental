use cgmath::{InnerSpace, Vector3};

use super::{ecef_to_globe_world, CelestialError, EcefDirection};
use crate::{coords::LatLon, projection::globe::lat_lon_to_unit_sphere};

/// The ECEF unit vector of a geocentric latitude and longitude, from the axes' definition.
fn ecef_from_lat_lon(latitude: f64, longitude: f64) -> Vector3<f64> {
    let (latitude, longitude) = (latitude.to_radians(), longitude.to_radians());
    Vector3::new(
        latitude.cos() * longitude.cos(),
        latitude.cos() * longitude.sin(),
        latitude.sin(),
    )
}

fn assert_close(actual: Vector3<f64>, expected: Vector3<f64>) {
    assert!(
        (actual - expected).magnitude() < 1e-12,
        "{actual:?} != {expected:?}"
    );
}

/// A wrong permutation turns the terminator: each subsolar point must land where the globe
/// draws that place.
#[test]
fn subsolar_points_land_where_the_globe_draws_them() {
    for (latitude, longitude, ecef) in [
        (0.0, 0.0, Vector3::unit_x()),
        (0.0, 90.0, Vector3::unit_y()),
        (90.0, 0.0, Vector3::unit_z()),
        (-35.0, -120.0, ecef_from_lat_lon(-35.0, -120.0)),
    ] {
        assert_close(ecef_from_lat_lon(latitude, longitude), ecef);
        let sun = EcefDirection::new(ecef.x, ecef.y, ecef.z).expect("unit vector");
        assert_close(
            sun.globe_world(),
            lat_lon_to_unit_sphere(LatLon::new(latitude, longitude)),
        );
        assert_close(
            ecef_to_globe_world(ecef),
            lat_lon_to_unit_sphere(LatLon::new(latitude, longitude)),
        );
    }
}

#[test]
fn directions_are_normalized_and_degenerate_ones_refused() {
    let sun = EcefDirection::new(0.0, 0.0, 3.0).expect("finite");
    assert_close(sun.ecef(), Vector3::unit_z());
    for (x, y, z) in [
        (0.0, 0.0, 0.0),
        (f64::NAN, 0.0, 1.0),
        (f64::INFINITY, 0.0, 0.0),
    ] {
        assert!(matches!(
            EcefDirection::new(x, y, z),
            Err(CelestialError::InvalidDirection { .. })
        ));
    }
}

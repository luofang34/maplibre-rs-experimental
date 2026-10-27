use cgmath::{InnerSpace, Vector3};

use super::{ecef_to_globe_world, CelestialError, CelestialLighting, EcefDirection, TwilightBand};
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

#[test]
fn twilight_bands_must_rise_between_finite_elevations() {
    assert!(TwilightBand::new(-6.0, 2.0, 0.1).is_ok());
    for (night_end, day_start, brightness) in [
        (2.0, -6.0, 0.1),
        (2.0, 2.0, 0.1),
        (f64::NAN, 2.0, 0.1),
        (-95.0, 2.0, 0.1),
        (-6.0, 2.0, 1.5),
    ] {
        assert!(matches!(
            TwilightBand::new(night_end, day_start, brightness),
            Err(CelestialError::InvalidTwilight { .. })
        ));
    }
}

#[test]
fn daylight_uniforms_carry_the_globe_sun_and_the_band_sines() {
    assert_eq!(
        CelestialLighting::default().daylight_uniforms()[0],
        [0.0; 4]
    );
    let lighting = CelestialLighting {
        sun: Some(EcefDirection::new(1.0, 0.0, 0.0).expect("direction")),
        twilight: TwilightBand::default(),
        atmosphere: false,
    };
    let [sun, twilight] = lighting.daylight_uniforms();
    assert_eq!(sun, [0.0, 0.0, 1.0, 1.0]);
    assert!((f64::from(twilight[0]) - (-6_f64).to_radians().sin()).abs() < 1e-6);
    assert!((f64::from(twilight[1]) - 2_f64.to_radians().sin()).abs() < 1e-6);
    assert_eq!(twilight[2], 0.1);
}

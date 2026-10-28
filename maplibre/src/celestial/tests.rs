use cgmath::{InnerSpace, Vector3};

use super::{
    ecef_to_globe_world, moonlight_uniform, CelestialError, CelestialLighting, Disc, EarthShadow,
    EcefDirection, TwilightBand, VisibleFraction, MOONLIGHT_GAIN, NIGHT_BRIGHTNESS,
};
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
        sun_visible: VisibleFraction::new(0.25).expect("fraction"),
    };
    let [sun, twilight] = lighting.daylight_uniforms();
    assert_eq!(sun, [0.0, 0.0, 1.0, 1.0]);
    assert!((f64::from(twilight[0]) - (-6_f64).to_radians().sin()).abs() < 1e-6);
    assert!((f64::from(twilight[1]) - 2_f64.to_radians().sin()).abs() < 1e-6);
    assert_eq!(twilight[2], NIGHT_BRIGHTNESS);
    assert_eq!(twilight[3], 0.25, "the share of the sun an eclipse leaves");
}

#[test]
fn the_whole_sun_is_visible_until_a_host_says_otherwise() {
    assert_eq!(CelestialLighting::default().sun_visible.get(), 1.0);
    assert_eq!(CelestialLighting::default().daylight_uniforms()[1][3], 1.0);
    for fraction in [-0.01, 1.01, f32::NAN] {
        assert!(
            VisibleFraction::new(fraction).is_err(),
            "{fraction} is refused"
        );
    }
}

#[test]
fn moonlight_is_the_moon_direction_and_its_lit_share() {
    let sun = EcefDirection::new(1.0, 0.0, 0.0).ok();
    let disc = |x, y, z| Disc::new(EcefDirection::new(x, y, z).expect("direction"), 0.26).ok();
    let full = moonlight_uniform(sun, disc(-1.0, 0.0, 0.0), None);
    assert_eq!(full, [0.0, 0.0, -1.0, MOONLIGHT_GAIN]);
    let quarter = moonlight_uniform(sun, disc(0.0, 1.0, 0.0), None);
    assert!((quarter[3] - MOONLIGHT_GAIN / 2.0).abs() < 1e-6);
    assert_eq!(
        moonlight_uniform(sun, disc(1.0, 0.0, 0.0), None)[3],
        0.0,
        "new Moon"
    );
    assert_eq!(
        moonlight_uniform(None, disc(-1.0, 0.0, 0.0), None),
        [0.0; 4]
    );
    assert_eq!(moonlight_uniform(sun, None, None), [0.0; 4]);
}

#[test]
fn the_earths_shadow_takes_the_moon_its_sunlight() {
    let axis = EcefDirection::new(1.0, 0.0, 0.0).expect("axis");
    let shadow = EarthShadow::new(axis, 0.7, 1.3, None).expect("shadow");
    assert_eq!(shadow.danjon(), 2.0, "L2 by default");
    let at = |degrees: f64| {
        let (sine, cosine) = degrees.to_radians().sin_cos();
        shadow.sunlit_share(EcefDirection::new(cosine, sine, 0.0).expect("direction"))
    };
    assert_eq!(at(0.0), 0.0, "the umbra's centre");
    assert_eq!(at(0.69), 0.0, "inside the umbra");
    assert!((at(1.0) - 0.5).abs() < 1e-6, "halfway across the penumbra");
    assert_eq!(at(1.31), 1.0, "beyond the penumbra");

    // A full moon on the shadow's axis sends the ground no light.
    let sun = EcefDirection::new(-1.0, 0.0, 0.0).ok();
    let moon = Disc::new(axis, 0.26).ok();
    assert_eq!(moonlight_uniform(sun, moon, Some(shadow))[3], 0.0);
    assert_eq!(moonlight_uniform(sun, moon, None)[3], MOONLIGHT_GAIN);
}

#[test]
fn an_earth_shadow_must_nest_its_umbra_in_its_penumbra() {
    let axis = EcefDirection::new(1.0, 0.0, 0.0).expect("axis");
    for (umbra, penumbra, danjon) in [
        (0.0, 1.3, None),
        (1.3, 0.7, None),
        (0.7, 5.1, None),
        (0.7, 1.3, Some(4.5)),
        (0.7, 1.3, Some(f32::NAN)),
    ] {
        assert!(
            EarthShadow::new(axis, umbra, penumbra, danjon).is_err(),
            "{umbra}°, {penumbra}°, {danjon:?}"
        );
    }
}

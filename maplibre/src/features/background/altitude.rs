//! Altitude-dependent atmosphere fade calculations matching MapLibre GL JS.

use cgmath::InnerSpace;

use crate::projection::globe::camera::GlobeCameraState;

/// Upper altitude threshold (100 km) where atmosphere scattering reaches full strength.
pub const ATMOSPHERE_FADE_ALTITUDE_METERS: f64 = 100_000.0;

/// Returns the smooth $C^1$-continuous Hermite fade factor in `[0.0, 1.0]` for the given camera altitude in meters.
///
/// Matches MapLibre GL JS `getAtmosphereAltitudeBlend`: $t^2 (3 - 2t)$, clamping to 0.0 at or below ground (<= 0.0 m)
/// and 1.0 at or above 100 km (100,000 m).
pub fn atmosphere_altitude_blend(altitude_meters: f64) -> f32 {
    if altitude_meters.is_nan() || altitude_meters <= 0.0 {
        0.0
    } else if altitude_meters >= ATMOSPHERE_FADE_ALTITUDE_METERS {
        1.0
    } else {
        let t = (altitude_meters / ATMOSPHERE_FADE_ALTITUDE_METERS) as f32;
        (t * t * (3.0 - 2.0 * t)).clamp(0.0, 1.0)
    }
}

/// Evaluates atmosphere altitude blend factor from a globe camera state.
///
/// Computes altitude above the body's sea-level radius using the unit-sphere camera distance:
/// `altitude_meters = (distance_from_center - 1.0) * body.radius_meters`.
pub fn altitude_blend_for_camera(camera: &GlobeCameraState) -> f32 {
    let distance_ratio = camera.camera_position().magnitude();
    let altitude_meters = (distance_ratio - 1.0) * camera.body().radius_meters;
    atmosphere_altitude_blend(altitude_meters)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ground_and_sub_msl_produce_zero_blend() {
        assert_eq!(atmosphere_altitude_blend(0.0), 0.0);
        assert_eq!(atmosphere_altitude_blend(-1.0), 0.0);
        assert_eq!(atmosphere_altitude_blend(-430.0), 0.0); // Dead Sea
        assert_eq!(atmosphere_altitude_blend(f64::NEG_INFINITY), 0.0);
        assert_eq!(atmosphere_altitude_blend(f64::NAN), 0.0);
    }

    #[test]
    fn space_altitudes_produce_full_blend() {
        assert_eq!(atmosphere_altitude_blend(100_000.0), 1.0);
        assert_eq!(atmosphere_altitude_blend(150_000.0), 1.0);
        assert_eq!(atmosphere_altitude_blend(1_000_000.0), 1.0);
        assert_eq!(atmosphere_altitude_blend(f64::INFINITY), 1.0);
    }

    #[test]
    fn hermite_smoothstep_midpoint_is_half() {
        // At t = 0.5 (50 km), t^2 * (3 - 2t) = 0.25 * 2.0 = 0.5
        let blend_50km = atmosphere_altitude_blend(50_000.0);
        assert!((blend_50km - 0.5).abs() < 1e-6);
    }

    #[test]
    fn hermite_smoothstep_quarter_and_three_quarters() {
        // At t = 0.25 (25 km), 0.0625 * 2.5 = 0.15625
        let blend_25km = atmosphere_altitude_blend(25_000.0);
        assert!((blend_25km - 0.15625).abs() < 1e-6);

        // At t = 0.75 (75 km), 0.5625 * 1.5 = 0.84375
        let blend_75km = atmosphere_altitude_blend(75_000.0);
        assert!((blend_75km - 0.84375).abs() < 1e-6);
    }

    #[test]
    fn strictly_monotonic_between_zero_and_threshold() {
        let mut prev = -1.0;
        for step in 0..=100 {
            let alt = step as f64 * 1_000.0;
            let blend = atmosphere_altitude_blend(alt);
            assert!(blend >= prev, "blend should be monotonic at alt {alt}");
            assert!((0.0..=1.0).contains(&blend));
            prev = blend;
        }
        assert_eq!(prev, 1.0);
    }

    #[test]
    fn camera_altitude_blend_evaluation() {
        use cgmath::Point2;

        use crate::{
            coords::LatLon,
            projection::{body::Body, globe::camera::GlobeCameraOptions},
        };

        // Create a camera with sea-level orbit
        let options = GlobeCameraOptions {
            width: 800.0,
            height: 600.0,
            field_of_view_degrees: 45.0,
            center: LatLon::new(0.0, 0.0),
            world_size: 512.0,
            bearing_degrees: 0.0,
            pitch_degrees: 0.0,
            roll_degrees: 0.0,
            center_offset: Point2::new(0.0, 0.0),
            body: Body::EARTH,
            target_elevation_meters: 0.0,
            radius_pixels: None,
        };
        let camera = GlobeCameraState::new(options).expect("valid camera");
        let blend = altitude_blend_for_camera(&camera);
        // At world_size 512, zoom is around 1 (outer space view, distance >> 100 km)
        assert_eq!(blend, 1.0);
    }
}

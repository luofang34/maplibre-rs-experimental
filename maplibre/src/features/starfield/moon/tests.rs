use cgmath::Vector3;

use super::{magnitude_at_phase_angle, relative_brightness};

#[test]
fn a_full_moon_is_brightest_and_a_crescent_far_fainter() {
    let moon = Vector3::new(0.0, 0.0, 1.0);
    // The sun behind the observer: a full moon, phase angle zero.
    let full = relative_brightness(Some(Vector3::new(0.0, 0.0, -1.0)), moon);
    assert!((full - 1.0).abs() < 1e-12);
    // A quarter: phase angle 90°, magnitude -10.13, about a tenth of full.
    let quarter = relative_brightness(Some(Vector3::new(1.0, 0.0, 0.0)), moon);
    assert!((magnitude_at_phase_angle(90.0) - -10.1276).abs() < 1e-3);
    assert!((0.085..0.1).contains(&quarter), "a quarter moon {quarter}");
    // A thin crescent, 30° from the sun: phase angle 150°.
    let (sine, cosine) = 30_f64.to_radians().sin_cos();
    let crescent = relative_brightness(Some(Vector3::new(sine, 0.0, cosine)), moon);
    assert!(crescent < 0.01, "a crescent {crescent}");
    assert_eq!(relative_brightness(None, moon), 1.0, "no sun draws it full");
}

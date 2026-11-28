use super::radius_pixels;

#[test]
fn the_sun_spans_its_angle_through_the_focal_length() {
    // A 90° field of view over 1000 pixels has a focal length of 500 pixels.
    let y_scale = 1.0 / 45_f64.to_radians().tan();
    let radius = radius_pixels(y_scale, 1000.0, 0.2626);
    assert!((radius - 500.0 * 0.2626_f64.to_radians().tan()).abs() < 1e-9);
    assert!(radius_pixels(y_scale * 2.0, 1000.0, 0.2626) > radius * 1.99);
}

use super::{dash_pixels, normalize_pattern};
#[test]
fn dash_distance_texture_contains_on_and_off_intervals_at_the_declared_ratio() {
    let (pixels, period) = dash_pixels(&[2.0, 1.0]);
    assert_eq!(period, 3.0);
    let on = pixels.chunks_exact(4).filter(|p| p[0] > 128).count();
    assert!((on as i32 - 170).abs() < 3);
    assert!(pixels[4 * 80] > 128);
    assert!(pixels[4 * 210] < 128);
}
#[test]
fn odd_patterns_repeat_with_alternating_gaps_and_invalid_patterns_are_solid() {
    assert_eq!(
        normalize_pattern(vec![2.0, 1.0, 3.0]),
        [2.0, 1.0, 3.0, 2.0, 1.0, 3.0]
    );
    assert!(normalize_pattern(vec![-1.0, 2.0]).is_empty());
    assert!(normalize_pattern(vec![0.0, 0.0]).is_empty());
}

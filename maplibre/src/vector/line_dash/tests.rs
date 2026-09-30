use super::{dash_pixels, normalize_pattern, round_dash_pixels, WIDTH};
#[test]
fn dash_distance_texture_contains_on_and_off_intervals_at_the_declared_ratio() {
    let (pixels, period) = dash_pixels(&[2.0, 1.0]);
    assert_eq!(period, 3.0);
    let on = pixels
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|p| p[0] > 128)
        .count();
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

#[test]
fn a_round_dash_ends_in_a_half_circle_across_the_line() {
    let (pixels, period, rows) = round_dash_pixels(&[1.0, 1.0]);
    assert_eq!(period, 2.0);
    let sample = |row: u32, x: u32| pixels[((row * WIDTH + x) * 4) as usize];
    let (centre, edge) = (rows / 2, 0);
    // A gap just past the end of a dash is covered by the cap at the middle of the line and
    // not at its edge.
    let just_past_dash = WIDTH / 2 + 6;
    assert!(sample(centre, just_past_dash) > 128);
    assert!(sample(edge, just_past_dash) < 128);
}

use std::{error::Error, time::Duration};

use serde_json::json;

use super::{
    build_dash_texture_pixels,
    crossfade::{
        dash_uv_per_tile_unit, eval_floor_width, evaluate_dash_patterns, parse_dasharray,
        ZoomHistory, DEFAULT_FADE_DURATION,
    },
    pixels::{dash_pixels, normalize_pattern, round_dash_pixels},
    WIDTH,
};
use crate::style::layer::LinePaint;
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
fn odd_patterns_join_their_end_dashes_and_zero_gaps_join_dashes() {
    assert_eq!(normalize_pattern(vec![2.0, 1.0, 3.0]), [5.0, 1.0]);
    assert_eq!(normalize_pattern(vec![1.0, 1.0, 0.0]), [1.0, 1.0]);
    assert_eq!(normalize_pattern(vec![1.0, 0.0, 1.0, 1.0]), [2.0, 1.0]);
    assert!(normalize_pattern(vec![1.0, 0.0, 1.0]).is_empty());
    assert!(normalize_pattern(vec![1.0, 0.0]).is_empty());
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

#[test]
fn fractional_zoom_dash_uv_is_invariant_for_interpolated_line_width() -> Result<(), Box<dyn Error>>
{
    let paint: LinePaint = serde_json::from_value(json!({
        "line-width": ["interpolate", ["exponential", 1.2], ["zoom"], 14, 0.5, 20, 10],
        "line-dasharray": [1, 0.75]
    }))?;

    assert!((eval_floor_width(&paint, 14.5) - 0.5).abs() < 1e-6);

    let baseline =
        dash_uv_per_tile_unit(&paint, 16.0, 16, None).ok_or("dash UV expected at baseline zoom")?;
    for z in [16.0_f64, 16.25, 16.5, 16.75, 16.99] {
        let uv =
            dash_uv_per_tile_unit(&paint, z, 16, None).ok_or("dash UV expected for screen tile")?;
        assert!(
            (uv - baseline).abs() < 1e-5,
            "screen dash UV drifted at z={z}: got {uv}, expected {baseline}"
        );
        let drape_uv = dash_uv_per_tile_unit(&paint, z, 16, Some(16.0))
            .ok_or("dash UV expected for drape tile")?;
        assert!(
            (drape_uv - baseline).abs() < 1e-5,
            "drape dash UV drifted at z={z}: got {drape_uv}, expected {baseline}"
        );
    }
    Ok(())
}

#[test]
fn dasharray_zoom_stops_are_stepped_not_interpolated() -> Result<(), Box<dyn Error>> {
    let prop = parse_dasharray(&json!({
        "stops": [
            [0, [1, 1]],
            [2, [2, 2]]
        ]
    }));
    let z1 = prop.evaluate_at_zoom(1.0).map(|v| v.0);
    assert_eq!(z1.as_deref(), Some(&[1.0, 1.0][..]));
    let z2 = prop.evaluate_at_zoom(2.0).map(|v| v.0);
    assert_eq!(z2.as_deref(), Some(&[2.0, 2.0][..]));

    let paint: LinePaint = serde_json::from_value(json!({
        "line-width": 4,
        "line-dasharray": {
            "stops": [[0, [1, 1]], [2, [2, 2]]]
        }
    }))?;
    let (from_p, to_p) = evaluate_dash_patterns(&paint, 2.0, true);
    assert_eq!(from_p, vec![1.0, 1.0]);
    assert_eq!(to_p, vec![2.0, 2.0]);
    Ok(())
}

#[test]
fn zoom_history_crossfade_parameters_match_gl_js() {
    let mut history = ZoomHistory::default();
    let t0 = Duration::from_millis(100);
    history.update(15.95, Some(t0));
    let settled = history.crossfade(15.95, Some(t0), DEFAULT_FADE_DURATION);
    assert_eq!(settled.fade_t, 1.0);
    assert_eq!(settled.mix, 1.0);

    // Cross integer boundary up (15.95 -> 16.05).
    let cross_up = Duration::from_millis(200);
    history.update(16.05, Some(cross_up));
    let at_start = history.crossfade(16.05, Some(cross_up), DEFAULT_FADE_DURATION);
    assert_eq!(at_start.from_scale, 2.0);
    assert_eq!(at_start.to_scale, 1.0);
    assert!((at_start.mix - 0.05).abs() < 1e-5);
    assert!((at_start.fade_t - 0.0).abs() < 1e-6);

    let half = history.crossfade(
        16.05,
        Some(cross_up + Duration::from_millis(150)),
        DEFAULT_FADE_DURATION,
    );
    assert!((half.fade_t - 0.5).abs() < 1e-5);
    assert!((half.mix - 0.525).abs() < 1e-5);

    let done = history.crossfade(
        16.05,
        Some(cross_up + DEFAULT_FADE_DURATION),
        DEFAULT_FADE_DURATION,
    );
    assert_eq!(done.fade_t, 1.0);
    assert_eq!(done.mix, 1.0);

    // Cross integer boundary down (16.05 -> 15.95).
    let cross_down = Duration::from_millis(600);
    history.update(15.95, Some(cross_down));
    let down_start = history.crossfade(15.95, Some(cross_down), DEFAULT_FADE_DURATION);
    assert_eq!(down_start.from_scale, 0.5);
    assert_eq!(down_start.to_scale, 1.0);
    assert!((down_start.mix - 0.05).abs() < 1e-5);

    let down_done = history.crossfade(
        15.95,
        Some(cross_down + DEFAULT_FADE_DURATION),
        DEFAULT_FADE_DURATION,
    );
    assert_eq!(down_done.fade_t, 1.0);
    assert_eq!(down_done.mix, 1.0);

    let no_clock = history.crossfade(15.95, None, DEFAULT_FADE_DURATION);
    assert_eq!(no_clock.fade_t, 1.0);
    assert_eq!(no_clock.mix, 1.0);
}

#[test]
fn dual_dash_texture_stacks_from_and_to_without_bleed() {
    let (single_pixels, p_from, p_to, rows, dual) =
        build_dash_texture_pixels(&[1.0, 1.0], &[1.0, 1.0], false);
    assert!(!dual);
    assert_eq!(rows, 1);
    assert_eq!(p_from, 2.0);
    assert_eq!(p_to, 2.0);
    assert_eq!(single_pixels.len(), WIDTH as usize * 4);

    let (pixels, from_period, to_period, rows, dual) =
        build_dash_texture_pixels(&[1.0, 1.0], &[2.0, 1.0], false);
    assert!(dual);
    assert_eq!(rows, 2);
    assert_eq!(from_period, 2.0);
    assert_eq!(to_period, 3.0);
    let row_bytes = WIDTH as usize * 4;
    assert_eq!(
        pixels.get(..row_bytes),
        Some(dash_pixels(&[1.0, 1.0]).0.as_slice())
    );
    assert_eq!(
        pixels.get(row_bytes..2 * row_bytes),
        Some(dash_pixels(&[2.0, 1.0]).0.as_slice())
    );

    let (_, _, _, round_rows, round_dual) =
        build_dash_texture_pixels(&[1.0, 1.0], &[2.0, 1.0], true);
    assert!(round_dual);
    assert_eq!(round_rows, 30);
}

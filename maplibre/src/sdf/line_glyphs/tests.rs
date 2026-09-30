#![allow(clippy::expect_used, clippy::panic)]
use super::*;

/// An arc of radius `radius` around the origin, as many short segments.
fn arc(radius: f32, from_degrees: f32, to_degrees: f32, steps: usize) -> Vec<[f32; 2]> {
    (0..=steps)
        .map(|step| {
            let degrees = from_degrees + (to_degrees - from_degrees) * step as f32 / steps as f32;
            let radians = degrees.to_radians();
            [radius * radians.cos(), radius * radians.sin()]
        })
        .collect()
}

#[test]
fn glyphs_on_a_straight_line_keep_their_distances_and_the_line_direction() {
    let line = [[0.0, 0.0], [1000.0, 0.0]];
    let poses = place_glyphs(&line, 500.0, &[-30.0, -10.0, 10.0, 30.0], false).expect("fits");
    let xs: Vec<f32> = poses.iter().map(|pose| pose.point[0]).collect();
    assert_eq!(xs, [470.0, 490.0, 510.0, 530.0]);
    assert!(poses
        .iter()
        .all(|pose| pose.angle == 0.0 && pose.point[1] == 0.0));
}

#[test]
fn glyphs_on_a_curve_lie_on_it_and_point_along_its_tangent() {
    let radius = 800.0;
    let line = arc(radius, 0.0, 90.0, 720);
    let quarter = radius * std::f32::consts::FRAC_PI_2;
    let offsets = [-200.0, -100.0, 0.0, 100.0, 200.0];
    let poses = place_glyphs(&line, quarter / 2.0, &offsets, false).expect("fits");
    for (pose, offset) in poses.iter().zip(offsets) {
        let distance = pose.point[0].hypot(pose.point[1]);
        assert!(
            (distance - radius).abs() < 0.5,
            "glyph {offset} at {distance}"
        );
        // Along the arc the angle of travel is the polar angle plus a quarter turn.
        let polar = pose.point[1].atan2(pose.point[0]);
        let expected = polar + std::f32::consts::FRAC_PI_2;
        assert!(
            (pose.angle - expected).abs() < 0.01,
            "{} vs {expected}",
            pose.angle
        );
    }
    // A straight baseline through the middle glyph would leave the outer ones far off the arc.
    let sagitta = radius - (radius * radius - 200.0 * 200.0).sqrt();
    assert!(sagitta > 25.0);
}

#[test]
fn flipping_reads_the_text_the_other_way_and_turns_each_glyph() {
    let line = [[0.0, 0.0], [1000.0, 0.0]];
    let offsets = [-20.0, 20.0];
    let forward = place_glyphs(&line, 500.0, &offsets, false).expect("fits");
    let flipped = place_glyphs(&line, 500.0, &offsets, true).expect("fits");
    assert_eq!(forward[0].point, flipped[1].point);
    assert_eq!(forward[1].point, flipped[0].point);
    assert!((flipped[0].angle - std::f32::consts::PI).abs() < 1e-6);
}

#[test]
fn a_label_longer_than_the_line_fits_nowhere() {
    let line = [[0.0, 0.0], [100.0, 0.0]];
    assert!(place_glyphs(&line, 50.0, &[-80.0, 80.0], false).is_none());
    assert!(place_glyphs(&line, 50.0, &[-20.0, 20.0], false).is_some());
}

#[test]
fn text_reads_backwards_when_it_runs_right_to_left_on_screen() {
    assert!(reads_backwards([300.0, 10.0], [100.0, 12.0]));
    assert!(!reads_backwards([100.0, 10.0], [300.0, 12.0]));
}

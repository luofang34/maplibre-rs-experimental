#![allow(clippy::expect_used, clippy::panic)]
use super::*;

/// A view that shows tile units `scale` times as pixels, with no perspective.
fn flat(scale: f64) -> impl Fn([f64; 2]) -> Option<OnScreen> {
    move |[x, y]| {
        Some(OnScreen {
            screen: [x * scale, y * scale],
            w: 1.0,
        })
    }
}

/// A view looking along +x: depth grows with x, so the far end of a line is foreshortened,
/// and points at x below `-eye` lie behind the eye.
fn receding(eye: f64) -> impl Fn([f64; 2]) -> Option<OnScreen> {
    move |[x, y]| {
        let w = 1.0 + x / eye;
        Some(OnScreen {
            screen: [400.0 + 300.0 * y / w, 500.0 - 200.0 * x / eye / w],
            w,
        })
    }
}

fn arc(radius: f32, steps: usize) -> Vec<[f32; 2]> {
    (0..=steps)
        .map(|step| {
            let angle = std::f32::consts::FRAC_PI_2 * step as f32 / steps as f32;
            [radius * angle.cos(), radius * angle.sin()]
        })
        .collect()
}

#[test]
fn glyphs_are_spaced_in_screen_pixels_and_turn_with_a_curve() {
    let radius = 1000.0;
    let line = arc(radius, 360);
    let quarter = radius * std::f32::consts::FRAC_PI_2;
    // Two pixels a tile unit: a glyph 100 px from the anchor lies 50 tile units along the arc.
    let poses = place_glyphs_on_screen(
        &line,
        quarter / 2.0,
        &[-100.0, 0.0, 100.0],
        false,
        flat(2.0),
    )
    .expect("fits");
    for (pose, offset) in poses.iter().zip([-100.0_f32, 0.0, 100.0]) {
        let [x, y] = pose.point;
        assert!((x.hypot(y) - radius).abs() < 0.5, "on the arc: {pose:?}");
        let polar = y.atan2(x);
        let along = polar * radius - quarter / 2.0;
        assert!(
            (along - offset / 2.0).abs() < 0.5,
            "{offset} px is {along} tile units along"
        );
        // Travel along the arc points a quarter turn past the polar angle.
        let expected = polar + std::f32::consts::FRAC_PI_2;
        assert!(
            (pose.angle - expected).abs() < 0.01,
            "{} vs {expected}",
            pose.angle
        );
    }
}

#[test]
fn foreshortened_glyphs_keep_their_pixel_spacing_and_lie_on_the_line() {
    let line = [[0.0, 0.0], [40000.0, 0.0]];
    let view = receding(1000.0);
    let offsets = [-60.0, -20.0, 20.0, 60.0];
    let poses = place_glyphs_on_screen(&line, 1000.0, &offsets, false, &view).expect("fits");
    let anchor = view([1000.0, 0.0]).expect("anchor").screen;
    for (pose, offset) in poses.iter().zip(offsets) {
        assert_eq!(pose.point[1], 0.0, "on the line");
        let at = view([f64::from(pose.point[0]), 0.0]).expect("glyph").screen;
        let pixels = (at[0] - anchor[0]).hypot(at[1] - anchor[1]);
        assert!(
            (pixels - offset.abs()).abs() < 1e-3,
            "{offset} px drawn {pixels} px away"
        );
        // The line runs up the screen, away from the eye.
        assert!((f64::from(pose.angle) + std::f64::consts::FRAC_PI_2).abs() < 1e-6);
    }
    // Equal pixel steps cover more of the line where it is farther away.
    let spans: Vec<f32> = poses
        .windows(2)
        .map(|pair| pair[1].point[0] - pair[0].point[0])
        .collect();
    assert!(spans.windows(2).all(|pair| pair[1] > pair[0]), "{spans:?}");
}

#[test]
fn a_flipped_label_walks_back_and_turns_half_way_around() {
    let line = [[0.0, 0.0], [1000.0, 0.0]];
    let forward =
        place_glyphs_on_screen(&line, 500.0, &[-10.0, 10.0], false, flat(1.0)).expect("fits");
    let flipped =
        place_glyphs_on_screen(&line, 500.0, &[-10.0, 10.0], true, flat(1.0)).expect("fits");
    assert_eq!(forward[0].point, [490.0, 0.0]);
    assert_eq!(flipped[0].point, [510.0, 0.0]);
    assert!(
        (flipped[0].angle.cos() + 1.0).abs() < 1e-6,
        "turned half way around"
    );
    assert!(forward[0].angle.abs() < 1e-6);
}

#[test]
fn a_label_longer_than_its_line_or_crossing_behind_the_eye_does_not_fit() {
    let line = [[0.0, 0.0], [100.0, 0.0]];
    assert!(place_glyphs_on_screen(&line, 50.0, &[-60.0, 60.0], false, flat(1.0)).is_none());
    // Behind the eye at x below -1000: a glyph reaching past the cut does not fit, one short
    // of it does.
    let long = [[-3000.0, 0.0], [3000.0, 0.0]];
    let view = receding(1000.0);
    let near = view([-999.0, 0.0]).expect("near").screen;
    let anchor = view([0.0, 0.0]).expect("anchor").screen;
    let reach = (near[0] - anchor[0]).hypot(near[1] - anchor[1]);
    assert!(place_glyphs_on_screen(&long, 3000.0, &[-(reach * 0.5)], false, &view).is_some());
    assert!(place_glyphs_on_screen(&long, 3000.0, &[-(reach * 2.0)], false, &view).is_none());
}

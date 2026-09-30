#![allow(clippy::expect_used, clippy::panic)]
use super::*;

fn params(spacing: f64, label_length: f64) -> AnchorSpacing {
    AnchorSpacing {
        spacing,
        max_angle: 45.0_f64.to_radians(),
        label_length,
        text_size: 100.0,
    }
}

#[test]
fn anchors_repeat_at_the_spacing_from_the_first_offset() {
    let line = vec![[100.0, 2000.0], [3900.0, 2000.0]];
    let anchors = line_anchors(&line, params(1000.0, 200.0));
    // The first sits half a label plus two glyphs in: (100 + 2 * 100) % 1000 = 300.
    let xs: Vec<f64> = anchors.iter().map(|anchor| anchor.point[0]).collect();
    assert_eq!(xs, [400.0, 1400.0, 2400.0, 3400.0]);
    assert!(anchors.iter().all(|anchor| anchor.angle == 0.0));
}

#[test]
fn labels_longer_than_the_spacing_push_the_repeats_apart() {
    let line = vec![[0.0, 2000.0], [4000.0, 2000.0]];
    let anchors = line_anchors(&line, params(300.0, 600.0));
    let gap = anchors[1].point[0] - anchors[0].point[0];
    assert_eq!(
        gap,
        600.0 + 300.0 / 4.0,
        "spacing grows to label plus a quarter"
    );
}

#[test]
fn a_line_continuing_across_the_tile_edge_starts_on_its_own_rhythm() {
    let starts_on_edge = vec![[0.0, 2000.0], [4000.0, 2000.0]];
    let anchors = line_anchors(&starts_on_edge, params(1000.0, 200.0));
    assert_eq!(anchors[0].point[0], 100.0, "offset is one glyph height");
}

#[test]
fn a_sharp_corner_under_the_label_refuses_it_until_the_limit_allows_it() {
    let bent = vec![[0.0, 2000.0], [2000.0, 2000.0], [2000.0, 4000.0]];
    let centre_of_corner = AnchorSpacing {
        spacing: 100_000.0,
        ..params(100_000.0, 400.0)
    };
    assert!(center_anchor(
        &[[1000.0, 2000.0], [2000.0, 2000.0], [2000.0, 3000.0]],
        centre_of_corner
    )
    .is_none());
    let generous = AnchorSpacing {
        max_angle: 100.0_f64.to_radians(),
        ..centre_of_corner
    };
    assert!(center_anchor(
        &[[1000.0, 2000.0], [2000.0, 2000.0], [2000.0, 3000.0]],
        generous
    )
    .is_some());
    // A gentle line accepts the same label.
    assert!(center_anchor(&[[0.0, 2000.0], [3000.0, 2000.0]], centre_of_corner).is_some());
    let _ = bent;
}

#[test]
fn a_short_line_gets_one_label_in_the_middle_when_the_rhythm_misses() {
    let line = vec![[1000.0, 2000.0], [1250.0, 2000.0]];
    let anchors = line_anchors(&line, params(1000.0, 200.0));
    assert_eq!(anchors.len(), 1);
    assert_eq!(anchors[0].point, [1125.0, 2000.0]);
}

#[test]
fn clipping_keeps_the_part_inside_the_tile_and_splits_at_reentry() {
    let line = vec![
        [-1000.0, 1000.0],
        [1000.0, 1000.0],
        [1000.0, -1000.0],
        [2000.0, -1000.0],
    ];
    let clipped = clip_to_tile(&[line]);
    assert_eq!(
        clipped,
        vec![vec![[0.0, 1000.0], [1000.0, 1000.0], [1000.0, 0.0]]]
    );
    let leaving = clip_to_tile(&[vec![[3000.0, 100.0], [5000.0, 100.0]]]);
    assert_eq!(leaving, vec![vec![[3000.0, 100.0], [4096.0, 100.0]]]);
}

#[test]
fn anchors_outside_the_tile_belong_to_the_neighbour() {
    let line = vec![[3000.0, 2000.0], [6000.0, 2000.0]];
    for anchor in line_anchors(&line, params(500.0, 100.0)) {
        assert!(anchor.point[0] < 4096.0, "{anchor:?}");
    }
}

#[test]
fn a_label_longer_than_the_line_is_left_out() {
    let line = vec![[1000.0, 2000.0], [1300.0, 2000.0]];
    assert!(line_anchors(&line, params(1000.0, 400.0)).is_empty());
}

#[test]
fn a_line_center_label_in_the_tile_buffer_belongs_to_the_neighbour() {
    let line = [[3500.0, 2000.0], [5500.0, 2000.0]];
    let params = AnchorSpacing {
        max_angle: 180.0_f64.to_radians(),
        ..params(1000.0, 200.0)
    };
    assert!(
        center_anchor(&line, params).is_none(),
        "the middle is at 4500"
    );
    assert!(center_anchor(&[[1000.0, 2000.0], [3000.0, 2000.0]], params).is_some());
}

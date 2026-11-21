#![allow(clippy::expect_used, clippy::panic)]
use super::*;

fn nine_part() -> AtlasEntry {
    AtlasEntry {
        rect: [100, 200, 40, 40],
        metrics: [0.0, 0.0, 0.0, 1.0],
        kind: 1,
        stretch: Some(Box::new(IconStretch {
            stretch_x: vec![[10.0, 30.0]],
            stretch_y: vec![[10.0, 30.0]],
            ..Default::default()
        })),
    }
}

#[test]
fn an_icon_without_a_fit_is_one_quad() {
    let quads = icon_quads(&nine_part(), [0.0, 0.0, 40.0, 40.0], false, 1.0);
    assert_eq!(quads.len(), 1);
    // The quad takes one texel of the clear border around the image on each side.
    assert_eq!(quads[0].rect, [99, 199, 42, 42]);
    assert_eq!(quads[0].bounds, [-1.0, -1.0, 41.0, 41.0]);
}

#[test]
fn a_nine_part_icon_keeps_its_corners_and_stretches_the_middle() {
    let quads = icon_quads(&nine_part(), [0.0, 0.0, 100.0, 60.0], true, 1.0);
    assert_eq!(quads.len(), 9);
    let corner = |rect: [u32; 4]| quads.iter().find(|quad| quad.rect == rect).expect("piece");
    let top_left = corner([100, 200, 10, 10]);
    assert_eq!(top_left.bounds, [0.0, 0.0, 10.0, 10.0]);
    let bottom_right = corner([130, 230, 10, 10]);
    assert_eq!(bottom_right.bounds, [90.0, 50.0, 100.0, 60.0]);
    let middle = corner([110, 210, 20, 20]);
    assert_eq!(middle.bounds, [10.0, 10.0, 90.0, 50.0]);
}

#[test]
fn a_content_area_stands_for_the_box_and_the_edges_lie_outside_it() {
    let mut icon = nine_part();
    if let Some(stretch) = icon.stretch.as_mut() {
        stretch.content = Some([10.0, 10.0, 30.0, 30.0]);
    }
    let quads = icon_quads(&icon, [0.0, 0.0, 100.0, 60.0], true, 1.0);
    let top_left = quads
        .iter()
        .find(|quad| quad.rect == [100, 200, 10, 10])
        .expect("piece");
    // The 10 pixel border sticks out of the box the text and its padding make.
    assert_eq!(top_left.bounds, [-10.0, -10.0, 0.0, 0.0]);
}

#[test]
fn standalone_stretchable_icon_scales_stretch_zones_and_keeps_fixed_corners() {
    let mut icon = nine_part();
    if let Some(stretch) = icon.stretch.as_mut() {
        stretch.content = Some([10.0, 10.0, 30.0, 30.0]);
    }
    // Standalone 40x40 icon at icon-size = 2.0 => 80x80 total box.
    let quads = icon_quads(&icon, [-40.0, -40.0, 40.0, 40.0], false, 2.0);
    assert_eq!(quads.len(), 9);
    let corner = |rect: [u32; 4]| quads.iter().find(|quad| quad.rect == rect).expect("piece");
    assert_eq!(corner([100, 200, 10, 10]).bounds, [-40.0, -40.0, -30.0, -30.0]);
    assert_eq!(corner([110, 210, 20, 20]).bounds, [-30.0, -30.0, 30.0, 30.0]);
    assert_eq!(corner([130, 230, 10, 10]).bounds, [30.0, 30.0, 40.0, 40.0]);
}

#[test]
fn fitted_stretchable_icon_scales_fixed_borders_by_icon_size_around_text_content() {
    let mut icon = nine_part();
    if let Some(stretch) = icon.stretch.as_mut() {
        stretch.content = Some([10.0, 10.0, 30.0, 30.0]);
    }
    // Text content box is [-15, -6, 15, 6] with icon-size = 2.0 => 10px borders scale to 20px.
    let quads = icon_quads(&icon, [-15.0, -6.0, 15.0, 6.0], true, 2.0);
    assert_eq!(quads.len(), 9);
    let corner = |rect: [u32; 4]| quads.iter().find(|quad| quad.rect == rect).expect("piece");
    assert_eq!(corner([110, 210, 20, 20]).bounds, [-15.0, -6.0, 15.0, 6.0]);
    assert_eq!(corner([100, 200, 10, 10]).bounds, [-35.0, -26.0, -15.0, -6.0]);
    assert_eq!(corner([130, 230, 10, 10]).bounds, [15.0, 6.0, 35.0, 26.0]);
}

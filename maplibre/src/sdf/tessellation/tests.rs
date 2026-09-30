#![allow(clippy::expect_used, clippy::panic)]
use geozero::{FeatureProcessor, GeomProcessor};

use super::*;
use crate::sdf::assets::{AtlasEntry, SymbolAtlas};

fn atlas() -> Arc<SymbolAtlas> {
    Arc::new(SymbolAtlas {
        glyphs: [(
            "test".into(),
            [(
                'A' as u32,
                AtlasEntry {
                    rect: [0, 0, 10, 18],
                    metrics: [0., -9., 10., 1.],
                    kind: 0,
                    ..Default::default()
                },
            )]
            .into(),
        )]
        .into(),
        ..Default::default()
    })
}

fn labelled(placement: &str, extra: serde_json::Value, line: &[[f64; 2]]) -> TextTessellator {
    let mut paint = serde_json::json!({
        "text-field": "AAAA", "text-font": ["test"], "text-size": 16,
        "symbol-placement": placement, "symbol-spacing": 100, "text-max-width": 1
    });
    for (key, value) in extra.as_object().expect("extra").clone() {
        paint[key] = value;
    }
    let paint: SymbolPaint = serde_json::from_value(paint).expect("paint");
    let mut tessellator = TextTessellator::default();
    tessellator.configure(paint, atlas());
    tessellator
        .linestring_begin(true, line.len(), 0)
        .expect("begin");
    for (index, point) in line.iter().enumerate() {
        tessellator.xy(point[0], point[1], index).expect("vertex");
    }
    tessellator.linestring_end(true, 0).expect("end");
    tessellator.feature_end(0).expect("feature");
    tessellator.finish();
    tessellator
}

fn anchors_x(tessellator: &TextTessellator) -> Vec<f32> {
    tessellator
        .features
        .iter()
        .map(|feature| feature.text_anchor.x)
        .collect()
}

#[test]
fn a_line_label_repeats_along_the_line_and_is_never_wrapped() {
    // 100 px of spacing is 800 tile units; the label is 4 glyphs of 10 at size 16.
    let tessellator = labelled(
        "line",
        serde_json::json!({}),
        &[[0.0, 2048.0], [4000.0, 2048.0]],
    );
    assert_eq!(
        anchors_x(&tessellator),
        [128.0, 928.0, 1728.0, 2528.0, 3328.0]
    );
    let second = tessellator.features[1].line.as_ref().expect("a line label");
    assert_eq!(second.anchor_distance, 928.0);
    assert_eq!(&*second.polyline, &[[0.0, 2048.0], [4000.0, 2048.0]]);
    assert_eq!(
        second.glyph_offsets,
        [-15.0, -5.0, 5.0, 15.0],
        "glyph centres around the label centre, in 24-pixel layout units"
    );
    for feature in &tessellator.features {
        assert_eq!(
            feature.indices.len(),
            4 * 6,
            "the four glyphs stay on one line despite the small max width"
        );
    }
}

#[test]
fn line_center_places_one_label_and_repeat_distance_follows_the_style() {
    let center = labelled(
        "line-center",
        serde_json::json!({}),
        &[[0.0, 2048.0], [4000.0, 2048.0]],
    );
    assert_eq!(anchors_x(&center), [2000.0]);
    let sparse = labelled(
        "line",
        serde_json::json!({"symbol-spacing": 200}),
        &[[0.0, 2048.0], [4000.0, 2048.0]],
    );
    assert_eq!(anchors_x(&sparse), [128.0, 1728.0, 3328.0]);
}

#[test]
fn every_part_of_a_multi_line_feature_is_labelled() {
    let mut tessellator = TextTessellator::default();
    tessellator.configure(
        serde_json::from_value(serde_json::json!({
            "text-field": "AAAA", "text-font": ["test"], "text-size": 16,
            "symbol-placement": "line-center"
        }))
        .expect("paint"),
        atlas(),
    );
    tessellator.multilinestring_begin(2, 0).expect("begin");
    for (part, y) in [(0, 1000.0), (1, 3000.0)] {
        tessellator.linestring_begin(false, 2, part).expect("part");
        tessellator.xy(500.0, y, 0).expect("vertex");
        tessellator.xy(2500.0, y, 1).expect("vertex");
        tessellator.linestring_end(false, part).expect("part end");
    }
    tessellator.multilinestring_end(0).expect("end");
    tessellator.feature_end(0).expect("feature");
    tessellator.finish();
    let ys: Vec<f32> = tessellator
        .features
        .iter()
        .map(|feature| feature.text_anchor.y)
        .collect();
    assert_eq!(
        ys,
        [1000.0, 3000.0],
        "each line has its own label, on the line"
    );
}

fn point_label(extra: serde_json::Value) -> TextTessellator {
    let mut paint = serde_json::json!({"text-field": "A", "text-font": ["test"], "text-size": 24});
    for (key, value) in extra.as_object().expect("extra").clone() {
        paint[key] = value;
    }
    let paint: SymbolPaint = serde_json::from_value(paint).expect("paint");
    let mut tessellator = TextTessellator::default();
    tessellator.configure(paint, atlas());
    tessellator.point_begin(0).expect("begin");
    tessellator.xy(100.0, 100.0, 0).expect("vertex");
    tessellator.point_end(0).expect("end");
    tessellator
        .property(0, "turn", &ColumnValue::Double(90.0))
        .expect("property");
    tessellator.feature_end(0).expect("feature");
    tessellator.finish();
    tessellator
}

fn corner_offsets(tessellator: &TextTessellator) -> Vec<[i32; 2]> {
    tessellator
        .quad_buffer
        .vertices
        .iter()
        .map(|vertex| [vertex.a_pos_offset[2], vertex.a_pos_offset[3]])
        .collect()
}

#[test]
fn text_rotation_can_come_from_a_feature_property() {
    let flat = corner_offsets(&point_label(serde_json::json!({})));
    let turned = corner_offsets(&point_label(
        serde_json::json!({"text-rotate": ["get", "turn"]}),
    ));
    // A quarter turn sends each offset (x, y) to (-y, x), clockwise on a y-down screen.
    for (before, after) in flat.iter().zip(&turned) {
        assert!((after[0] + before[1]).abs() <= 1 && (after[1] - before[0]).abs() <= 1);
    }
    assert_ne!(flat, turned);
}

#[test]
fn icon_text_fit_stretches_the_icon_around_the_text_and_padding() {
    let mut symbols = atlas().as_ref().clone();
    symbols.icons.insert(
        "box".into(),
        AtlasEntry {
            rect: [0, 0, 20, 20],
            metrics: [0., 0., 20., 1.],
            kind: 1,
            ..Default::default()
        },
    );
    let paint: SymbolPaint = serde_json::from_value(serde_json::json!({
        "text-field": "A", "text-font": ["test"], "text-size": 24, "icon-image": "box",
        "icon-text-fit": "both", "icon-text-fit-padding": [1, 2, 3, 4]
    }))
    .expect("paint");
    let mut tessellator = TextTessellator::default();
    tessellator.configure(paint, Arc::new(symbols));
    tessellator.point_begin(0).expect("begin");
    tessellator.xy(100.0, 100.0, 0).expect("vertex");
    tessellator.point_end(0).expect("end");
    tessellator.feature_end(0).expect("feature");
    tessellator.finish();
    let corner = |index: usize| {
        let vertex = &tessellator.quad_buffer.vertices[index];
        [
            vertex.a_pos_offset[2] as f32 / 32.0,
            vertex.a_pos_offset[3] as f32 / 32.0,
        ]
    };
    // The text box is 10 wide and one 28.8 line high around the anchor.
    let (top_left, bottom_right) = (corner(0), corner(2));
    assert!((top_left[0] + 9.0).abs() < 0.1 && (top_left[1] + 15.4).abs() < 0.1);
    assert!((bottom_right[0] - 7.0).abs() < 0.1 && (bottom_right[1] - 17.4).abs() < 0.1);
}

#[test]
fn variable_anchors_give_the_shifts_from_the_first_anchor_to_each_other_one() {
    let mut tessellator = point_label(serde_json::json!({
        "text-variable-anchor": ["top", "bottom", "left", "right"]
    }));
    // A text 10 wide and 28.8 high: the laid-out one hangs below the point.
    let shifts = tessellator.features.remove(0).anchor_shifts;
    let expected = [[0.0, 0.0], [0.0, -28.8], [5.0, -14.4], [-5.0, -14.4]];
    assert_eq!(shifts.len(), 4);
    for (shift, want) in shifts.iter().zip(expected) {
        assert!((shift[0] - want[0]).abs() < 1e-4 && (shift[1] - want[1]).abs() < 1e-4);
    }
}

#[test]
fn auto_justification_lays_the_text_out_once_for_each_justification_its_anchors_need() {
    let mut tessellator = point_label(serde_json::json!({
        "text-variable-anchor": ["left", "right", "top-left", "bottom"], "text-justify": "auto"
    }));
    let feature = tessellator.features.remove(0);
    assert_eq!(feature.text_sets.len(), 3);
    assert_eq!(feature.anchor_sets, [0, 1, 0, 2]);
    assert!(feature
        .text_sets
        .windows(2)
        .all(|pair| pair[0].end == pair[1].start));
}

#[test]
fn a_radial_offset_pushes_each_anchor_away_from_the_point() {
    let mut tessellator = point_label(serde_json::json!({
        "text-variable-anchor": ["top", "left"], "text-radial-offset": 1
    }));
    let shifts = tessellator.features.remove(0).anchor_shifts;
    // Top sits 24 below the point; left sits 24 to its right and centred: from top that is
    // 5 right (half the width) and 14.4 up, plus the change of offset.
    assert!((shifts[1][0] - (5.0 + 24.0)).abs() < 1e-3);
    assert!((shifts[1][1] - (-14.4 - 24.0)).abs() < 1e-3);
}

#[test]
fn polygon_rings_are_wound_clockwise_on_screen_outside_and_counter_clockwise_for_holes() {
    use geo_types::{LineString, Polygon};

    // In tile coordinates y grows downwards, so this outer ring runs counter-clockwise.
    let outer = LineString::from(vec![(0.0, 0.0), (0.0, 10.0), (10.0, 10.0), (10.0, 0.0)]);
    let hole = LineString::from(vec![(2.0, 2.0), (8.0, 2.0), (8.0, 8.0), (2.0, 8.0)]);
    let rings = super::polygon_rings(&Polygon::new(outer, vec![hole]));
    assert_eq!(
        rings[0],
        [
            [0.0, 0.0],
            [10.0, 0.0],
            [10.0, 10.0],
            [0.0, 10.0],
            [0.0, 0.0]
        ],
        "the outer ring is reversed"
    );
    assert_eq!(
        rings[1],
        [[2.0, 2.0], [2.0, 8.0], [8.0, 8.0], [8.0, 2.0], [2.0, 2.0]],
        "the hole, clockwise as given, is reversed too"
    );
}

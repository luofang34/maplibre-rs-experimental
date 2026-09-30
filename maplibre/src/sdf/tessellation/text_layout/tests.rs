#![allow(clippy::expect_used, clippy::panic)]
use super::*;

#[test]
fn letter_spacing_is_between_glyphs_and_does_not_shift_centered_text() {
    let paint: SymbolPaint = serde_json::from_value(serde_json::json!({
        "text-field":"AA", "text-letter-spacing":0.1, "text-font":["test"]
    }))
    .expect("paint");
    let atlas = SymbolAtlas {
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
    };
    let symbol = CollectedSymbol {
        id: None,
        line: None,
        anchor: geo_types::Point::new(100., 100.),
        properties: Default::default(),
        angle: 0.,
    };
    let mut buffer = VertexBuffers::new();
    append(&symbol, &paint, 12., &atlas, &mut buffer);
    let left = buffer
        .vertices
        .iter()
        .map(|v| v.a_pos_offset[2])
        .min()
        .expect("left");
    let right = buffer
        .vertices
        .iter()
        .map(|v| v.a_pos_offset[2])
        .max()
        .expect("right");
    assert_eq!(
        left + right,
        0,
        "a centered label must not include a trailing tracking gap"
    );
    assert!(((right - left) as f32 / 32.0 - 22.4).abs() < 0.07);
}

fn label_top(anchor: &str, field: &str) -> i32 {
    let paint: SymbolPaint = serde_json::from_value(serde_json::json!({
        "text-field": field, "text-font": ["test"], "text-max-width": 0.5, "text-anchor": anchor
    }))
    .expect("paint");
    let atlas = SymbolAtlas {
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
    };
    let symbol = CollectedSymbol {
        id: None,
        line: None,
        anchor: geo_types::Point::new(100., 100.),
        properties: Default::default(),
        angle: 0.,
    };
    let mut buffer = VertexBuffers::new();
    append(&symbol, &paint, 12., &atlas, &mut buffer);
    buffer
        .vertices
        .iter()
        .map(|v| v.a_pos_offset[3])
        .min()
        .expect("top")
}

#[test]
fn every_line_takes_a_full_line_height_when_anchoring_a_block() {
    // "A A" wraps into two lines at a one-em width; each line is 1.2 ems of 24 pixels tall.
    let block = 2.0 * 1.2 * 24.0;
    let shift = (label_top("top", "A A") - label_top("bottom", "A A")) as f32 / 32.0;
    assert!(
        (shift - block).abs() < 0.1,
        "a top-anchored block hangs {shift} px below a bottom-anchored one, not {block}"
    );
    let one = (label_top("top", "A") - label_top("bottom", "A")) as f32 / 32.0;
    assert!((one - 1.2 * 24.0).abs() < 0.1, "{one}");
}

fn advances(width: f32) -> HashMap<u32, AtlasEntry> {
    "abcdefghijklmnopqrstuvwxyz ()-"
        .chars()
        .map(|c| {
            let metrics = [0., 0., if c == ' ' { width / 2.0 } else { width }, 0.];
            (
                c as u32,
                AtlasEntry {
                    metrics,
                    ..Default::default()
                },
            )
        })
        .collect()
}

#[test]
fn text_breaks_where_the_lines_come_out_closest_in_width() {
    let glyphs = advances(10.0);
    // 150 wide over a 100 limit is two lines aiming for 75 each: breaking after the first word
    // leaves 40 and 100, closer than any other way to split.
    let lines = wrap("aaaa bbbbbbbb cc", 100.0, &glyphs, 0.0);
    assert_eq!(lines, ["aaaa", "bbbbbbbb cc"]);
    let joined = wrap("aa bb cc dd", 1000.0, &glyphs, 0.0);
    assert_eq!(joined, ["aa bb cc dd"], "a wide limit keeps one line");
}

#[test]
fn a_newline_always_breaks_and_a_zero_width_puts_each_word_on_its_own_line() {
    let glyphs = advances(10.0);
    assert_eq!(wrap("aa\nbb", 1000.0, &glyphs, 0.0), ["aa", "bb"]);
    assert_eq!(wrap("aa bb cc", 0.0, &glyphs, 0.0), ["aa", "bb", "cc"]);
}

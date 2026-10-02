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
        source: Default::default(),
        line: None,
        anchor: geo_types::Point::new(100., 100.),
        properties: Default::default(),
        angle: 0.,
        vertical: None,
        fallback: false,
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
        source: Default::default(),
        line: None,
        anchor: geo_types::Point::new(100., 100.),
        properties: Default::default(),
        angle: 0.,
        vertical: None,
        fallback: false,
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

#[test]
fn a_bidirectional_mark_is_not_drawn_even_where_the_font_has_a_glyph_for_it() {
    // GL JS's bidirectional pass leaves U+200E out of the text it draws; the fixture fonts
    // carry a visible glyph for it, so laying it out would draw a mark.
    let entry = AtlasEntry {
        rect: [0, 0, 10, 18],
        metrics: [0., -9., 10., 1.],
        kind: 0,
        ..Default::default()
    };
    let atlas = SymbolAtlas {
        glyphs: [(
            "test".into(),
            [('A' as u32, entry.clone()), ('\u{200E}' as u32, entry)].into(),
        )]
        .into(),
        ..Default::default()
    };
    let symbol = CollectedSymbol {
        source: Default::default(),
        line: None,
        anchor: geo_types::Point::new(100., 100.),
        properties: Default::default(),
        angle: 0.,
        vertical: None,
        fallback: false,
    };
    let laid_out = |field: &str| {
        let paint: SymbolPaint = serde_json::from_value(serde_json::json!({
            "text-field": field, "text-font": ["test"]
        }))
        .expect("paint");
        let mut buffer = VertexBuffers::new();
        append(&symbol, &paint, 12., &atlas, &mut buffer);
        buffer
            .vertices
            .iter()
            .map(|vertex| vertex.a_pos_offset)
            .collect::<Vec<_>>()
    };
    assert_eq!(laid_out("A\u{200E}A"), laid_out("AA"));
}

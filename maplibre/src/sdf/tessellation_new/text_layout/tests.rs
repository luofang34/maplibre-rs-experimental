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
                },
            )]
            .into(),
        )]
        .into(),
        ..Default::default()
    };
    let symbol = CollectedSymbol {
        id: None,
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

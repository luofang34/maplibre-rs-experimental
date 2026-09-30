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

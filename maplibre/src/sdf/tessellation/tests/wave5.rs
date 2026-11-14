//! Unit tests for `symbol-avoid-edges` and OpenType `GSUB`/`GPOS` shaped glyph layout in
//! [`TextTessellator`].

use std::sync::Arc;

use geozero::{FeatureProcessor, GeomProcessor};

use super::super::*;
use crate::sdf::assets::{AtlasEntry, SymbolAtlas};

fn collect_points(
    paint: serde_json::Value,
    atlas: Arc<SymbolAtlas>,
    pts: &[[f64; 2]],
) -> TextTessellator {
    let paint: SymbolPaint = serde_json::from_value(paint).expect("paint");
    let mut tess = TextTessellator::default();
    tess.configure(paint, atlas);
    for (idx, pt) in pts.iter().enumerate() {
        tess.point_begin(idx).expect("point_begin");
        tess.xy(pt[0], pt[1], idx).expect("xy");
        tess.point_end(idx).expect("point_end");
        tess.feature_end(idx as u64).expect("feature_end");
    }
    tess.finish();
    tess
}

#[test]
fn symbol_avoid_edges_drops_edge_crossing_point_and_line_labels_and_keeps_interior() {
    // 4 glyphs of advance 10 at size 24 -> width 40 px = 320 tile units (+/- 160 around center).
    let base_atlas = super::atlas();

    let without_avoid = collect_points(
        serde_json::json!({
            "text-field": "AAAA",
            "text-font": ["test"],
            "text-size": 24,
            "symbol-avoid-edges": false
        }),
        Arc::clone(&base_atlas),
        &[[50.0, 2048.0], [2048.0, 2048.0], [4060.0, 2048.0]],
    );
    assert_eq!(
        without_avoid.features.len(),
        3,
        "all 3 point labels are emitted when symbol-avoid-edges is false"
    );

    let with_avoid = collect_points(
        serde_json::json!({
            "text-field": "AAAA",
            "text-font": ["test"],
            "text-size": 24,
            "symbol-avoid-edges": true
        }),
        Arc::clone(&base_atlas),
        &[[50.0, 2048.0], [2048.0, 2048.0], [4060.0, 2048.0]],
    );
    assert_eq!(
        with_avoid.features.len(),
        1,
        "left-edge and right-edge crossing point labels are suppressed"
    );
    assert_eq!(with_avoid.features[0].text_anchor.x, 2048.0);
    assert_eq!(with_avoid.quad_buffer.indices.len(), 4 * 6);

    // Variable anchor near left tile boundary: `right` crosses x < 0, `left` stays inside [0, 4096],
    // so `layout` retains the feature for `first_fitting_anchor` to select `left`.
    let variable_near_left = collect_points(
        serde_json::json!({
            "text-field": "AAAA",
            "text-font": ["test"],
            "text-size": 24,
            "text-variable-anchor": ["right", "left"],
            "symbol-avoid-edges": true
        }),
        Arc::clone(&base_atlas),
        &[[40.0, 2048.0]],
    );
    assert_eq!(
        variable_near_left.features.len(),
        1,
        "feature kept because `left` variable anchor fits inside [0, 4096]"
    );

    // Line-center near x = 4070 crosses x > 4096 and is dropped when `symbol-avoid-edges: true`,
    // whereas an interior line-center at x = 2048 is kept.
    let edge_line = super::labelled(
        "line-center",
        serde_json::json!({ "text-size": 24, "symbol-avoid-edges": true }),
        &[[4020.0, 2048.0], [4095.0, 2048.0]],
    );
    assert!(
        edge_line.features.is_empty(),
        "line-center label crossing x=4096 must be suppressed"
    );

    let interior_line = super::labelled(
        "line-center",
        serde_json::json!({ "text-size": 24, "symbol-avoid-edges": true }),
        &[[1500.0, 2048.0], [2500.0, 2048.0]],
    );
    assert_eq!(interior_line.features.len(), 1);
}

#[test]
fn shaped_opentype_runs_emit_single_ligature_and_indic_conjunct_quads() {
    let mut atlas = SymbolAtlas::default();
    atlas.face_glyphs.insert(
        "7:6".into(),
        AtlasEntry {
            rect: [2, 2, 20, 22],
            metrics: [-1.0, -14.0, 16.0, 1.0],
            kind: 0,
            ..Default::default()
        },
    );
    atlas.face_glyphs.insert(
        "7:10".into(),
        AtlasEntry {
            rect: [26, 2, 22, 24],
            metrics: [-1.0, -15.0, 18.0, 1.0],
            kind: 0,
            ..Default::default()
        },
    );
    let mut runs = std::collections::HashMap::new();
    runs.insert(
        "fi".to_owned(),
        vec![crate::font::ShapedGlyph {
            face_id: 7,
            glyph_id: 6,
            cluster: 0,
            x_advance: 16.0,
            y_advance: 0.0,
            x_offset: 0.0,
            y_offset: 0.0,
            rtl: false,
        }],
    );
    runs.insert(
        "\u{0915}\u{094D}\u{0937}".to_owned(),
        vec![crate::font::ShapedGlyph {
            face_id: 7,
            glyph_id: 10,
            cluster: 0,
            x_advance: 18.0,
            y_advance: 0.0,
            x_offset: 1.5,
            y_offset: 2.0,
            rtl: false,
        }],
    );
    atlas.shaped_runs.insert("CustomOT".into(), runs);
    let shared = Arc::new(atlas);

    let fi_tess = collect_points(
        serde_json::json!({
            "text-field": "fi",
            "text-font": ["CustomOT"],
            "text-size": 24
        }),
        Arc::clone(&shared),
        &[[2048.0, 2048.0]],
    );
    assert_eq!(fi_tess.features.len(), 1);
    assert_eq!(
        fi_tess.quad_buffer.indices.len(),
        6,
        "`fi` ligature emits 1 quad (6 indices) rather than 2 unjoined char quads"
    );

    let ksha_tess = collect_points(
        serde_json::json!({
            "text-field": "\u{0915}\u{094D}\u{0937}",
            "text-font": ["CustomOT"],
            "text-size": 24
        }),
        shared,
        &[[2048.0, 2048.0]],
    );
    assert_eq!(ksha_tess.features.len(), 1);
    assert_eq!(
        ksha_tess.quad_buffer.indices.len(),
        6,
        "Devanagari `क्ष` conjunct emits 1 shaped glyph quad (6 indices)"
    );
}

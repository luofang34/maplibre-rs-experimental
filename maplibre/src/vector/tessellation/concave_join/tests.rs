use geozero::{FeatureProcessor, GeomProcessor};

use crate::{
    style::{
        layer::StyleProperty,
        line_stroke::{LineJoin, LineStroke},
    },
    vector::tessellation::ZeroTessellator,
};

fn norm_len(n: [f32; 2]) -> f32 {
    (n[0] * n[0] + n[1] * n[1]).sqrt()
}

#[test]
fn sharp_coastal_hairpin_splits_concave_miter_without_spike_or_pinch_for_both_turn_directions() {
    for (join, limit) in [
        (LineJoin::Round, 2.001_f32),
        (LineJoin::Bevel, 2.001_f32),
        (LineJoin::Miter, 4.001_f32),
    ] {
        // Test both left-hand and right-hand ~1.15° hairpins whose edge lengths (200 tile units)
        // defeat Lyon's 0.5-unit path-space fold heuristic and emit |normal| ≈ 100 without our fix.
        for sign in [1.0_f64, -1.0_f64] {
            let mut tess = ZeroTessellator::<u32> {
                stroke: LineStroke {
                    join,
                    miter_limit: 2.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            tess.linestring_begin(true, 3, 0).unwrap();
            tess.xy(100.0, 100.0, 0).unwrap();
            tess.xy(300.0, 100.0, 1).unwrap();
            tess.xy(100.0, 100.0 + sign * 4.0, 2).unwrap();
            tess.linestring_end(true, 0).unwrap();
            tess.feature_end(0).unwrap();

            for (i, v) in tess.buffer.vertices.iter().enumerate() {
                let len = norm_len(v.normal);
                assert!(
                    len <= limit,
                    "join={join:?} sign={sign}: vertex {i} emitted spike normal {:?} (len={len})",
                    v.normal
                );
                assert!(
                    len >= 0.999,
                    "join={join:?} sign={sign}: vertex {i} pinched stroke normal {:?} (len={len})",
                    v.normal
                );
            }
        }
    }
}

#[test]
fn moderate_miter_corner_remains_unclipped_and_unpinched() {
    // A 45° interior angle produces |normal| = 1 / sin(22.5°) ≈ 2.613, which is within Lyon's
    // default miter threshold (2.0 * miter_limit = 4.0) on both convex and concave sides.
    let mut tess = ZeroTessellator::<u32> {
        stroke: LineStroke {
            join: LineJoin::Miter,
            miter_limit: 2.0,
            ..Default::default()
        },
        ..Default::default()
    };
    tess.linestring_begin(true, 3, 0).unwrap();
    tess.xy(100.0, 100.0, 0).unwrap();
    tess.xy(300.0, 100.0, 1).unwrap();
    tess.xy(158.58, 241.42, 2).unwrap();
    tess.linestring_end(true, 0).unwrap();
    tess.feature_end(0).unwrap();

    let max_seen = tess
        .buffer
        .vertices
        .iter()
        .map(|v| norm_len(v.normal))
        .fold(0.0_f32, f32::max);
    assert!(
        (2.55..=2.70).contains(&max_seen),
        "expected unclipped ~2.613 miter normal at 45° corner, got {max_seen}"
    );
}

#[test]
fn closed_polygon_ring_with_hairpin_at_start_vertex_eliminates_both_join_and_closing_spikes() {
    // Hairpin placed right at vertex 0 of a closed polygon stroked on a line layer, exercising
    // Lyon's `close()` duplicate start-edge vertex path.
    let ring = [
        (300.0, 100.0),
        (100.0, 101.5),
        (100.0, 200.0),
        (300.0, 100.0),
    ];

    let mut line_tess = ZeroTessellator::<u32> {
        is_line_layer: true,
        stroke: LineStroke {
            join: LineJoin::Round,
            miter_limit: 2.0,
            ..Default::default()
        },
        ..Default::default()
    };
    line_tess.polygon_begin(true, 1, 0).unwrap();
    line_tess.linestring_begin(false, ring.len(), 0).unwrap();
    for (idx, &(x, y)) in ring.iter().enumerate() {
        line_tess.xy(x, y, idx).unwrap();
    }
    line_tess.linestring_end(false, 0).unwrap();
    line_tess.polygon_end(true, 0).unwrap();
    line_tess.feature_end(0).unwrap();

    for (i, v) in line_tess.buffer.vertices.iter().enumerate() {
        let len = norm_len(v.normal);
        assert!(
            (0.999..=2.001).contains(&len),
            "closed line-layer vertex {i} has abnormal normal {:?} (len={len})",
            v.normal
        );
    }

    // Also verify `fill-outline-color` on the exact same sharp coastal polygon.
    let mut fill_tess = ZeroTessellator::<u32> {
        outline_property: Some(StyleProperty::Constant(
            csscolorparser::parse("#1b85b8").unwrap(),
        )),
        ..Default::default()
    };
    fill_tess.polygon_begin(true, 1, 0).unwrap();
    fill_tess.linestring_begin(false, ring.len(), 0).unwrap();
    for (idx, &(x, y)) in ring.iter().enumerate() {
        fill_tess.xy(x, y, idx).unwrap();
    }
    fill_tess.linestring_end(false, 0).unwrap();
    fill_tess.polygon_end(true, 0).unwrap();
    fill_tess.feature_end(0).unwrap();

    let max_outline = 2.0 * (0.5 * 8.0 / 3.0) + 1e-3;
    for (i, v) in fill_tess.buffer.vertices.iter().enumerate() {
        let len = norm_len(v.normal);
        assert!(
            len <= max_outline,
            "fill-outline vertex {i} emitted spike normal {:?} (len={len}, max={max_outline})",
            v.normal
        );
    }
}

//! A hill between the eye and a road name hides the name as a whole when it stands in front of
//! the name's anchor, and leaves the whole name drawn when it stands only before its tail, as
//! GL JS judges a label at its anchor: no word loses some of its glyphs.

use super::*;

/// How high a hill stands above the ground around it.
const HILL_METRES: f64 = 400.0;

/// Flat ground with a hill south of the road, between the road and the eye looking north,
/// spanning `east` metres east of the view's center.
fn hill(east: std::ops::Range<f64>) -> impl Fn([f64; 2]) -> f64 {
    move |[x, south]| {
        let before_road = (150.0..350.0).contains(&south);
        BASE_METRES
            + if before_road && east.contains(&x) {
                HILL_METRES
            } else {
                0.0
            }
    }
}

const HILL_VIEW: View = View {
    pitch: 60.0,
    ..VIEW
};

#[tokio::test]
async fn a_hill_before_only_a_names_tail_leaves_every_glyph_drawn() {
    let mut failures = Vec::new();
    for ratio in [1.0, 2.0] {
        let view = View { ratio, ..HILL_VIEW };
        let mut open = scene_map(view, &hill(0.0..0.0)).await;
        let open_pixels = open.settle().await;
        let reference = glyph_pixels(&open, &open_pixels, ratio);
        let mut behind = scene_map(view, &hill(40.0..450.0)).await;
        let pixels = behind.settle().await;
        failures.extend(incomplete_glyphs(
            &format!("{ratio}x, a hill before the east half"),
            &glyph_pixels(&behind, &pixels, ratio),
            &reference,
            0.8,
        ));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn a_hill_before_a_names_anchor_hides_every_glyph() {
    let mut failures = Vec::new();
    for ratio in [1.0, 2.0] {
        let view = View { ratio, ..HILL_VIEW };
        // A narrow hill covers the anchor and the glyphs either side of it; a wide one the
        // whole name.
        for east in [-60.0..60.0, -450.0..450.0] {
            let mut map = scene_map(view, &hill(east.clone())).await;
            let pixels = map.settle().await;
            let glyphs = glyph_pixels(&map, &pixels, ratio);
            if glyphs.iter().any(|count| *count > 2) {
                failures.push(format!(
                    "{ratio}x, a hill over {east:?} m: glyphs still show {glyphs:?}"
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

//! A road name climbing a slope draws every glyph as fully as on flat ground, from any view,
//! across a DEM tile's edge, at any pixel ratio, after a resize and over imagery.

use super::*;

/// The ground rising by `grade` per metre eastward through [`BASE_METRES`] at the center.
fn slope(grade: f64) -> impl Fn([f64; 2]) -> f64 {
    move |[east, _]| BASE_METRES + grade * east
}

/// The share of the road name's drawn text pixels at which a rendered-symbol query finds it,
/// which the collision boxes answer: below one where the boxes miss the glyphs as drawn.
fn queried_share(map: &SymbolMap, pixels: &[u8], ratio: f64) -> f64 {
    let width = map.map.head_texture().expect("color").width() as usize;
    let text: Vec<[f64; 2]> = pixels
        .chunks_exact(4)
        .enumerate()
        .filter(|(_, pixel)| {
            pixel
                .iter()
                .zip(NAME)
                .all(|(have, want)| have.abs_diff(want) < 60)
        })
        .map(|(index, _)| [(index % width) as f64 + 0.5, (index / width) as f64 + 0.5])
        .collect();
    let found = text
        .iter()
        .filter(|[x, y]| {
            !map.map
                .query_rendered_symbols([x / ratio, y / ratio], Some(&["road-name"]))
                .is_empty()
        })
        .count();
    found as f64 / text.len().max(1) as f64
}

/// Each glyph of the road name on a slope of `grade` against the same glyph on flat ground,
/// and whether queries find the name wherever its text is drawn.
async fn compare_with_flat(view: View, grade: f64) -> Vec<String> {
    let mut flat = scene_map(view, &slope(0.0)).await;
    let flat_pixels = flat.settle().await;
    let reference = glyph_pixels(&flat, &flat_pixels, view.ratio);
    let mut sloped = scene_map(view, &slope(grade)).await;
    let pixels = sloped.settle().await;
    let glyphs = glyph_pixels(&sloped, &pixels, view.ratio);
    let case = format!("{view:?} grade {grade}");
    // A glyph on the slope is seen a little more or less obliquely than on flat ground.
    let mut failures = incomplete_glyphs(&case, &glyphs, &reference, 0.6);
    let share = queried_share(&sloped, &pixels, view.ratio);
    if share < 0.995 {
        failures.push(format!(
            "{case}: queries find the name at {share:.3} of its text"
        ));
    }
    failures
}

#[tokio::test]
async fn every_glyph_of_a_road_name_shows_on_a_slope_from_any_view() {
    let mut failures = Vec::new();
    for grade in [0.15, -0.15] {
        for (pitch, bearing) in [(50.0, 0.0), (60.0, 25.0), (30.0, -40.0), (0.0, 0.0)] {
            let view = View {
                pitch,
                bearing,
                ..VIEW
            };
            failures.extend(compare_with_flat(view, grade).await);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn a_road_name_across_a_dem_tiles_edge_keeps_every_glyph() {
    // Zoom-15 DEM tiles meet under the middle of the zoom-14 tile the label is centred in.
    let mut failures = Vec::new();
    for grade in [0.15, -0.15] {
        let view = View {
            dem_zoom: 15,
            ..VIEW
        };
        failures.extend(compare_with_flat(view, grade).await);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn a_road_name_on_a_slope_keeps_every_glyph_at_every_pixel_ratio_and_after_a_resize() {
    let mut failures = Vec::new();
    for ratio in [2.0, 3.0] {
        failures.extend(compare_with_flat(View { ratio, ..VIEW }, 0.15).await);
    }
    let view = View { ratio: 2.0, ..VIEW };
    let size = crate::window::PhysicalSize::new(768, 512).expect("size");
    let mut flat = scene_map(view, &slope(0.0)).await;
    flat.settle().await;
    flat.map.resize(size);
    let flat_pixels = flat.settle().await;
    let mut sloped = scene_map(view, &slope(0.15)).await;
    sloped.settle().await;
    sloped.map.resize(size);
    let pixels = sloped.settle().await;
    failures.extend(incomplete_glyphs(
        "resized at 2x",
        &glyph_pixels(&sloped, &pixels, 2.0),
        &glyph_pixels(&flat, &flat_pixels, 2.0),
        0.6,
    ));
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn a_road_name_on_a_slope_keeps_every_glyph_over_imagery() {
    let view = View {
        imagery: true,
        ..VIEW
    };
    let failures = compare_with_flat(view, 0.15).await;
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

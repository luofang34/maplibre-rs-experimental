//! Spherical raster composition across mesh borders and source-tile changes.

use super::*;

pub(super) fn globe_style(coords: WorldTileCoords) -> Style {
    let mut style = alpha_style(false, 1.0, Some(128));
    let n = 2_f64.powi(i32::from(u8::from(coords.z)));
    let lon = (f64::from(coords.x) + 0.5) / n * 360.0 - 180.0;
    let lat = (std::f64::consts::PI * (1.0 - 2.0 * (f64::from(coords.y) + 0.5) / n))
        .sinh()
        .atan()
        .to_degrees();
    style.center = Some([lon, lat]);
    style.zoom = Some(f64::from(u8::from(coords.z)) + 0.125);
    style.projection = Some(crate::projection::ProjectionSpecification {
        projection_type: crate::projection::ProjectionType::VerticalPerspective,
    });
    style
}

pub(super) fn assert_spherical(map: &HeadlessMap) {
    let actual = crate::render::projection::projection_data_for_view(
        &map.map_context.style,
        &map.map_context.view_state,
    )
    .expect("projection");
    assert_eq!(actual.transition, 1.0, "exercise spherical shader path");
}

fn assert_source_color(bytes: &[u8]) {
    for y in 16..SIZE - 16 {
        for x in 16..SIZE - 16 {
            let start = ((y * SIZE + x) * 4) as usize;
            let pixel = &bytes[start..start + 4];
            let matches = [[128, 0, 127, 255], [0, 128, 127, 255]]
                .iter()
                .any(|color| pixel.iter().zip(color).all(|(a, b)| a.abs_diff(*b) <= 2));
            assert!(matches, "source overlap or gap at ({x},{y}): {pixel:?}");
        }
    }
}

mod tests;

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

/// Every pixel shows the parent's or a child's colour once. A pixel on the edge of a child that
/// has loaded over its parent may resolve its samples to a mix of the two, which lies between
/// them; a gap shows neither, and an overlap blends the child over the parent, off that line.
fn assert_source_color(bytes: &[u8]) {
    const PARENT: [f64; 3] = [128.0, 0.0, 127.0];
    const CHILD: [f64; 3] = [0.0, 128.0, 127.0];
    for y in 16..SIZE - 16 {
        for x in 16..SIZE - 16 {
            let start = ((y * SIZE + x) * 4) as usize;
            let pixel = &bytes[start..start + 4];
            let share = f64::from(pixel[1]) / 128.0;
            let mix = [0, 1, 2].map(|i| PARENT[i] + (CHILD[i] - PARENT[i]) * share);
            let matches = pixel[3] == 255
                && (0.0..=1.0).contains(&share)
                && pixel[..3]
                    .iter()
                    .zip(mix)
                    .all(|(a, b)| (f64::from(*a) - b).abs() <= 2.5);
            assert!(matches, "source overlap or gap at ({x},{y}): {pixel:?}");
        }
    }
}

mod tests;

//! One-pixel borders around DEM tiles so slope shading reads across tile edges.
//!
//! A tile's border holds the edge samples of its loaded neighbours, as GL JS backfills DEM
//! tiles, and its own edge where a neighbour is missing. Shaders that read these textures
//! address the interior at an offset of one texel.

use image::RgbaImage;

/// Texels added on each side of a DEM tile.
pub const BORDER: u32 = 1;

/// The image with a border of neighbour samples; `neighbours` is indexed `[dy + 1][dx + 1]`
/// and the centre entry is ignored. A neighbour of another size counts as missing.
pub fn with_border(image: &RgbaImage, neighbours: &[[Option<&RgbaImage>; 3]; 3]) -> RgbaImage {
    let (width, height) = image.dimensions();
    let edge = i64::from(BORDER);
    RgbaImage::from_fn(width + 2 * BORDER, height + 2 * BORDER, |x, y| {
        let (column, row) = (i64::from(x) - edge, i64::from(y) - edge);
        let side = |value: i64, size: u32| match value {
            v if v < 0 => -1,
            v if v >= i64::from(size) => 1,
            _ => 0,
        };
        let (dx, dy) = (side(column, width), side(row, height));
        let wrapped = |value: i64, size: u32| value.rem_euclid(i64::from(size)) as u32;
        let clamped = |value: i64, size: u32| value.clamp(0, i64::from(size) - 1) as u32;
        if (dx, dy) != (0, 0) {
            if let Some(neighbour) = neighbours[(dy + 1) as usize][(dx + 1) as usize]
                .filter(|neighbour| neighbour.dimensions() == (width, height))
            {
                return *neighbour.get_pixel(wrapped(column, width), wrapped(row, height));
            }
        }
        *image.get_pixel(clamped(column, width), clamped(row, height))
    })
}

/// A pixel value for tests that need to tell tiles apart.
#[cfg(test)]
fn solid(value: u8) -> RgbaImage {
    use image::Rgba;
    RgbaImage::from_pixel(4, 4, Rgba([value, 0, 0, 255]))
}

#[cfg(test)]
mod tests {
    use image::Rgba;

    use super::*;

    #[test]
    fn the_border_takes_neighbour_edges_and_replicates_where_none_loaded() {
        let centre = RgbaImage::from_fn(4, 4, |x, y| Rgba([(x * 10 + y) as u8, 0, 0, 255]));
        let east = solid(200);
        let mut neighbours: [[Option<&RgbaImage>; 3]; 3] = Default::default();
        neighbours[1][2] = Some(&east);
        let bordered = with_border(&centre, &neighbours);
        assert_eq!(bordered.dimensions(), (6, 6));
        assert_eq!(
            bordered.get_pixel(3, 3),
            centre.get_pixel(2, 2),
            "interior is offset"
        );
        assert_eq!(
            bordered.get_pixel(5, 3)[0],
            200,
            "east border is the east tile"
        );
        assert_eq!(
            bordered.get_pixel(0, 3),
            centre.get_pixel(0, 2),
            "an absent neighbour repeats the edge"
        );
        assert_eq!(
            bordered.get_pixel(5, 0),
            centre.get_pixel(3, 0),
            "an absent corner repeats the nearest interior texel"
        );
    }

    #[test]
    fn a_neighbour_of_another_size_is_ignored() {
        let centre = solid(7);
        let wrong = RgbaImage::from_pixel(2, 2, Rgba([99, 0, 0, 255]));
        let mut neighbours: [[Option<&RgbaImage>; 3]; 3] = Default::default();
        neighbours[1][0] = Some(&wrong);
        assert_eq!(with_border(&centre, &neighbours).get_pixel(0, 2)[0], 7);
    }
}

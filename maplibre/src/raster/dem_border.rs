//! One-pixel borders around DEM tiles so slope shading reads across tile edges.
//!
//! A tile's border holds the edge samples of its loaded neighbours, as GL JS backfills DEM
//! tiles, and its own edge where a neighbour is missing. Shaders that read these textures
//! address the interior at an offset of one texel.

use image::RgbaImage;

/// Texels added on each side of a DEM tile.
pub const BORDER: u32 = 1;

/// The bit standing for the neighbour at offset (`dx`, `dy`) in a sides mask.
pub fn side_bit(dx: i32, dy: i32) -> u16 {
    1 << ((dy + 1) * 3 + (dx + 1))
}

/// The sides of a border filled from a neighbour of the tile's own size.
pub fn filled_sides(image: &RgbaImage, neighbours: &[[Option<&RgbaImage>; 3]; 3]) -> u16 {
    let mut sides = 0;
    for dy in -1..=1_i32 {
        for dx in -1..=1_i32 {
            let usable = neighbours[(dy + 1) as usize][(dx + 1) as usize]
                .is_some_and(|neighbour| neighbour.dimensions() == image.dimensions());
            if (dx, dy) != (0, 0) && usable {
                sides |= side_bit(dx, dy);
            }
        }
    }
    sides
}

/// The image with a border of neighbour samples; `neighbours` is indexed `[dy + 1][dx + 1]`
/// and the centre entry is ignored. A neighbour of another size counts as missing.
///
/// The interior is copied row by row: this runs on the main thread for every DEM tile a
/// hillshade uploads, where a per-texel closure over a 512² tile costs milliseconds.
pub fn with_border(image: &RgbaImage, neighbours: &[[Option<&RgbaImage>; 3]; 3]) -> RgbaImage {
    let (width, height) = image.dimensions();
    let (out_width, out_height) = (width + 2 * BORDER, height + 2 * BORDER);
    let texel = |x: u32, y: u32| border_texel(image, neighbours, x, y);
    let mut bordered = RgbaImage::new(out_width, out_height);
    let source = image.as_raw();
    let row_bytes = width as usize * 4;
    let interior_offset = BORDER as usize * 4;
    for y in 0..out_height {
        let interior_row = y.checked_sub(BORDER).filter(|row| *row < height);
        let Some(row) = interior_row else {
            for x in 0..out_width {
                bordered.put_pixel(x, y, texel(x, y));
            }
            continue;
        };
        let start = (y * out_width) as usize * 4 + interior_offset;
        let from = row as usize * row_bytes;
        let target: &mut [u8] = &mut bordered;
        target[start..start + row_bytes].copy_from_slice(&source[from..from + row_bytes]);
        for x in (0..BORDER).chain(width + BORDER..out_width) {
            bordered.put_pixel(x, y, texel(x, y));
        }
    }
    bordered
}

/// The texel at `(x, y)` of the bordered image.
fn border_texel(
    image: &RgbaImage,
    neighbours: &[[Option<&RgbaImage>; 3]; 3],
    x: u32,
    y: u32,
) -> image::Rgba<u8> {
    let (width, height) = image.dimensions();
    let edge = i64::from(BORDER);
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
    fn every_texel_is_the_one_its_position_names() {
        let tile = |seed: u32| {
            RgbaImage::from_fn(7, 7, move |x, y| {
                Rgba([(x * 31 + y * 7 + seed) as u8, seed as u8, y as u8, 255])
            })
        };
        let images: Vec<RgbaImage> = (0..9).map(tile).collect();
        let wrong = RgbaImage::from_pixel(3, 3, Rgba([1, 2, 3, 4]));
        for present in [0_u16, 0b1_1111_1111, 0b1_0101_0101, 0b0_1010_1010] {
            let mut neighbours: [[Option<&RgbaImage>; 3]; 3] = Default::default();
            for (index, image) in images.iter().enumerate() {
                if present & (1 << index) != 0 {
                    neighbours[index / 3][index % 3] = Some(image);
                }
            }
            neighbours[0][1] = neighbours[0][1].or(Some(&wrong));
            let centre = &images[4];
            let bordered = with_border(centre, &neighbours);
            for (x, y, texel) in bordered.enumerate_pixels() {
                assert_eq!(
                    *texel,
                    border_texel(centre, &neighbours, x, y),
                    "texel ({x}, {y}) with neighbours {present:#b}"
                );
            }
        }
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

//! Resamples an `image` source into the tiles that cover it.
//!
//! GL JS draws the image as a projective quad; resampling it into raster tiles gives the same
//! picture and lets terrain, globe and masks treat it like any other raster source.

use image::RgbaImage;

use crate::coords::WorldTileCoords;

/// Pixels per side of a resampled tile.
pub const IMAGE_TILE_SIZE: u32 = 512;

type Matrix = [[f64; 3]; 3];

fn mercator([longitude, latitude]: [f64; 2]) -> [f64; 2] {
    let x = (longitude + 180.0) / 360.0;
    let y = 0.5
        - (std::f64::consts::FRAC_PI_4 + latitude.to_radians() / 2.0)
            .tan()
            .ln()
            / std::f64::consts::TAU;
    [x, y]
}

/// The projective map from the unit square to the four points, corner order top left, top
/// right, bottom right, bottom left.
fn unit_square_to_quad(p: [[f64; 2]; 4]) -> Matrix {
    let [p0, p1, p2, p3] = p;
    let (dx1, dx2) = (p1[0] - p2[0], p3[0] - p2[0]);
    let (dy1, dy2) = (p1[1] - p2[1], p3[1] - p2[1]);
    let dx3 = p0[0] - p1[0] + p2[0] - p3[0];
    let dy3 = p0[1] - p1[1] + p2[1] - p3[1];
    let (g, h) = if dx3 == 0.0 && dy3 == 0.0 {
        (0.0, 0.0)
    } else {
        let denominator = dx1 * dy2 - dy1 * dx2;
        (
            (dx3 * dy2 - dy3 * dx2) / denominator,
            (dx1 * dy3 - dy1 * dx3) / denominator,
        )
    };
    [
        [p1[0] - p0[0] + g * p1[0], p3[0] - p0[0] + h * p3[0], p0[0]],
        [p1[1] - p0[1] + g * p1[1], p3[1] - p0[1] + h * p3[1], p0[1]],
        [g, h, 1.0],
    ]
}

fn inverse(m: Matrix) -> Option<Matrix> {
    let cofactor = |r: usize, c: usize| {
        let (r1, r2) = ((r + 1) % 3, (r + 2) % 3);
        let (c1, c2) = ((c + 1) % 3, (c + 2) % 3);
        m[r1][c1] * m[r2][c2] - m[r1][c2] * m[r2][c1]
    };
    let determinant =
        m[0][0] * cofactor(0, 0) + m[0][1] * cofactor(0, 1) + m[0][2] * cofactor(0, 2);
    if determinant.abs() < f64::EPSILON * 1e-6 {
        return None;
    }
    let mut result = [[0.0; 3]; 3];
    for (r, row) in result.iter_mut().enumerate() {
        for (c, value) in row.iter_mut().enumerate() {
            *value = cofactor(c, r) / determinant;
        }
    }
    Some(result)
}

fn sample(image: &RgbaImage, u: f64, v: f64) -> [f32; 4] {
    let (width, height) = image.dimensions();
    let x = (u * f64::from(width) - 0.5).clamp(0.0, f64::from(width - 1));
    let y = (v * f64::from(height) - 0.5).clamp(0.0, f64::from(height - 1));
    let (x0, y0) = (x.floor() as u32, y.floor() as u32);
    let (x1, y1) = ((x0 + 1).min(width - 1), (y0 + 1).min(height - 1));
    let (fx, fy) = ((x - f64::from(x0)) as f32, (y - f64::from(y0)) as f32);
    let mut premultiplied = [0.0_f32; 4];
    for ((px, py), weight) in [
        ((x0, y0), (1.0 - fx) * (1.0 - fy)),
        ((x1, y0), fx * (1.0 - fy)),
        ((x0, y1), (1.0 - fx) * fy),
        ((x1, y1), fx * fy),
    ] {
        let [r, g, b, a] = image
            .get_pixel(px, py)
            .0
            .map(|channel| f32::from(channel) / 255.0);
        for (out, value) in premultiplied.iter_mut().zip([r * a, g * a, b * a, a]) {
            *out += value * weight;
        }
    }
    premultiplied
}

/// Copies of the world an image can wrap onto before the rest are ignored.
const MAX_WORLD_SHIFTS: usize = 4;

/// The part of the image inside `tile`, or `None` when the image does not touch it.
pub fn render_tile(
    image: &RgbaImage,
    coordinates: [[f64; 2]; 4],
    tile: WorldTileCoords,
) -> Option<RgbaImage> {
    let tile_count = 2_f64.powi(i32::from(u8::from(tile.z)));
    let corners = coordinates.map(mercator);
    let (west, east) = corners.iter().fold((f64::MAX, f64::MIN), |(lo, hi), c| {
        (lo.min(c[0]), hi.max(c[0]))
    });
    let (north, south) = corners.iter().fold((f64::MAX, f64::MIN), |(lo, hi), c| {
        (lo.min(c[1]), hi.max(c[1]))
    });
    let (left, top) = (
        f64::from(tile.x) / tile_count,
        f64::from(tile.y) / tile_count,
    );
    let size = 1.0 / tile_count;
    // The image may reach past the antimeridian, so the tile also shows it shifted by whole worlds.
    let shifts: Vec<f64> = ((west - left - size).floor() as i64..=(east - left).ceil() as i64)
        .map(|shift| shift as f64)
        .filter(|shift| {
            let (lo, hi) = (left + shift, left + shift + size);
            !(east < lo || west > hi)
        })
        .take(MAX_WORLD_SHIFTS)
        .collect();
    if shifts.is_empty() || south < top || north > top + size {
        return None;
    }
    let to_unit = inverse(unit_square_to_quad(corners))?;
    let mut tile_image = RgbaImage::new(IMAGE_TILE_SIZE, IMAGE_TILE_SIZE);
    let step = size / f64::from(IMAGE_TILE_SIZE);
    for (px, py, pixel) in tile_image.enumerate_pixels_mut() {
        let y = top + (f64::from(py) + 0.5) * step;
        for shift in &shifts {
            let x = left + shift + (f64::from(px) + 0.5) * step;
            let w = to_unit[2][0] * x + to_unit[2][1] * y + to_unit[2][2];
            let u = (to_unit[0][0] * x + to_unit[0][1] * y + to_unit[0][2]) / w;
            let v = (to_unit[1][0] * x + to_unit[1][1] * y + to_unit[1][2]) / w;
            if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) || w <= 0.0 {
                continue;
            }
            let [r, g, b, a] = sample(image, u, v);
            if a > 0.0 {
                *pixel = image::Rgba([r / a, g / a, b / a, a].map(|c| (c * 255.0).round() as u8));
                break;
            }
        }
    }
    Some(tile_image)
}

#[cfg(test)]
mod tests;

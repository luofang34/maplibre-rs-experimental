//! Image normalization and comparison for render tests.

use std::path::{Path, PathBuf};

use image::{ImageBuffer, Rgba, RgbaImage};

use crate::TestMeta;

pub(crate) fn composite_opaque_background(path: &Path, background: [u8; 3]) -> Result<(), String> {
    let mut image = image::open(path)
        .map_err(|error| format!("Cannot open actual for compositing: {error}"))?
        .to_rgba8();
    for pixel in image.pixels_mut() {
        let [red, green, blue, alpha_byte] = &mut pixel.0;
        let alpha = u16::from(*alpha_byte);
        let inverse_alpha = 255 - alpha;
        let [bg_red, bg_green, bg_blue] = background;
        for (channel, bg_channel) in [(red, bg_red), (green, bg_green), (blue, bg_blue)] {
            let blended = u16::from(*channel) * alpha + u16::from(bg_channel) * inverse_alpha;
            *channel = (blended / 255) as u8;
        }
        *alpha_byte = 255;
    }
    image
        .save(path)
        .map_err(|error| format!("Cannot save composited actual: {error}"))
}

/// Turns the premultiplied frame into straight colours, the form a browser exports a canvas
/// in and therefore the form the expected images hold.
pub(crate) fn unpremultiply(path: &Path) -> Result<(), String> {
    let mut image = image::open(path)
        .map_err(|error| format!("Cannot open actual for unpremultiplying: {error}"))?
        .to_rgba8();
    for pixel in image.pixels_mut() {
        let [red, green, blue, alpha_byte] = &mut pixel.0;
        let alpha = u32::from(*alpha_byte);
        if alpha == 0 || alpha == 255 {
            continue;
        }
        for channel in [red, green, blue] {
            *channel = ((u32::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8;
        }
    }
    image
        .save(path)
        .map_err(|error| format!("Cannot save unpremultiplied actual: {error}"))
}

/// A channel difference at least this large means a pixel shows another colour altogether, not
/// a shifted or antialiased edge.
const WRONG_PIXEL_CHANNEL_DIFFERENCE: u8 = 128;

/// Half the side of the square, in logical pixels, that must be wrong throughout for its centre
/// to count as part of a solid wrong region. Wider than an edge offset or a glyph stroke, so
/// only areas that are drawn wholly differently, such as holes in a fill, add up.
const SOLID_REGION_RADIUS: f64 = 4.0;

/// How two images differ.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Difference {
    /// Normalized mean channel difference in `[0, 1]`.
    pub(crate) mean: f64,
    /// Pixels at the centre of a square of wrong pixels; see [`SOLID_REGION_RADIUS`].
    pub(crate) solid: u64,
}

/// Pixels at the centre of a wholly wrong square a frame at one physical pixel per logical pixel
/// may hold: a few for a wrongly placed icon corner, while a region about 13 px across or more,
/// such as a hole in a fill, exceeds it.
const MAX_SOLID_PIXELS: f64 = 16.0;

impl Difference {
    /// The largest difference a fixture allows, its mean being `max_diff`.
    pub(crate) fn limits(max_diff: f64, pixel_ratio: f64) -> Self {
        Self {
            mean: max_diff,
            solid: (MAX_SOLID_PIXELS * pixel_ratio * pixel_ratio).round() as u64,
        }
    }

    /// Whether the difference stays below the mean limit and within the solid region one.
    pub(crate) fn within(self, limits: Self) -> bool {
        self.mean < limits.mean && self.solid <= limits.solid
    }
}

impl std::fmt::Display for Difference {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "diff={:.4}, solid={}", self.mean, self.solid)
    }
}

/// Writes a diff PNG and measures how the images differ.
pub(crate) fn compare_and_diff(
    actual_path: &Path,
    expected_path: &Path,
    diff_path: &Path,
    pixel_ratio: f64,
) -> Result<Difference, String> {
    let actual = image::open(actual_path)
        .map_err(|error| format!("Cannot open actual: {error}"))?
        .to_rgba8();
    let expected = image::open(expected_path)
        .map_err(|error| format!("Cannot open expected: {error}"))?
        .to_rgba8();
    if actual.dimensions() != expected.dimensions() {
        let (actual_width, actual_height) = actual.dimensions();
        let (expected_width, expected_height) = expected.dimensions();
        let diff = ImageBuffer::from_pixel(
            actual_width.max(expected_width),
            actual_height.max(expected_height),
            Rgba([255_u8, 0, 0, 255]),
        );
        diff.save(diff_path)
            .map_err(|error| format!("Cannot save dimension-mismatch diff: {error}"))?;
        return Err(format!(
            "Dimension mismatch: actual {actual_width}x{actual_height} vs expected {expected_width}x{expected_height}"
        ));
    }
    compare_equal_dimensions(&actual, &expected, diff_path, pixel_ratio)
}

fn compare_equal_dimensions(
    actual: &RgbaImage,
    expected: &RgbaImage,
    diff_path: &Path,
    pixel_ratio: f64,
) -> Result<Difference, String> {
    let (width, height) = actual.dimensions();
    let mut diff_image = RgbaImage::new(width, height);
    let mut total_diff = 0_u64;
    let mut wrong = Vec::with_capacity(width as usize * height as usize);
    for (x, y, actual_pixel) in actual.enumerate_pixels() {
        let expected_pixel = expected.get_pixel(x, y);
        let [ar, ag, ab, aa] = actual_pixel.0;
        let [er, eg, eb, ea] = expected_pixel.0;
        let channel_diffs = [
            ar.abs_diff(er),
            ag.abs_diff(eg),
            ab.abs_diff(eb),
            aa.abs_diff(ea),
        ];
        let max_channel = channel_diffs.iter().copied().max().unwrap_or(0);
        wrong.push(max_channel >= WRONG_PIXEL_CHANNEL_DIFFERENCE);
        total_diff += channel_diffs
            .iter()
            .map(|difference| u64::from(*difference))
            .sum::<u64>();
        let diff_pixel = if max_channel == 0 {
            Rgba([0_u8, 0, 0, 0])
        } else {
            Rgba([255_u8, 0, 0, max_channel])
        };
        diff_image.put_pixel(x, y, diff_pixel);
    }
    diff_image
        .save(diff_path)
        .map_err(|error| format!("Cannot save image diff: {error}"))?;
    let channel_count = u64::from(width) * u64::from(height) * 4;
    let radius = (SOLID_REGION_RADIUS * pixel_ratio).round().max(1.0) as usize;
    Ok(Difference {
        mean: total_diff as f64 / (channel_count as f64 * 255.0),
        solid: solid_pixels(&wrong, width as usize, radius),
    })
}

/// Counts the pixels whose whole square of side `2 * radius + 1` is `wrong`, `wrong` holding one
/// flag per pixel in rows of `width`.
fn solid_pixels(wrong: &[bool], width: usize, radius: usize) -> u64 {
    let height = wrong.len().checked_div(width).unwrap_or(0);
    let side = 2 * radius + 1;
    if width < side || height < side {
        return 0;
    }
    // Wrong pixels above and left of each corner, so any square's count is four lookups.
    let stride = width + 1;
    let mut sums = vec![0_u32; stride * (height + 1)];
    for (y, wrong_row) in wrong.chunks_exact(width).enumerate() {
        let mut row = 0_u32;
        for (x, &is_wrong) in wrong_row.iter().enumerate() {
            row += u32::from(is_wrong);
            let above = sums.get(y * stride + x + 1).copied().unwrap_or(0);
            if let Some(cell) = sums.get_mut((y + 1) * stride + x + 1) {
                *cell = above + row;
            }
        }
    }
    let sum_at = |r: usize, c: usize| sums.get(r * stride + c).copied().unwrap_or(0);
    let full = (side * side) as u32;
    let mut solid = 0_u64;
    for top in 0..=height - side {
        for left in 0..=width - side {
            let (bottom, right) = (top + side, left + side);
            let count = sum_at(bottom, right) + sum_at(top, left)
                - sum_at(top, right)
                - sum_at(bottom, left);
            solid += u64::from(count == full);
        }
    }
    solid
}

/// Moves the final rendered frame to `actual.png`, normalizes it, and compares against references.
pub(crate) fn compare_frame_blocking(
    frame: &Path,
    test_dir: &Path,
    meta: &TestMeta,
) -> Result<Difference, String> {
    if !frame.exists() {
        return Err("Renderer did not produce the requested final frame".into());
    }
    let actual = test_dir.join("actual.png");
    std::fs::rename(frame, &actual)
        .map_err(|error| format!("Cannot move rendered frame into test output: {error}"))?;
    unpremultiply(&actual)?;
    if meta.show_padding {
        camera_options::draw_overlay(
            &actual,
            meta.padding,
            (meta.width, meta.height),
            meta.pixel_ratio,
        )?;
    }
    if let Some(background) = meta.comparison_background {
        composite_opaque_background(&actual, background)?;
    }
    // GL JS fixtures carry alternative reference images for platform or precision variants
    // (`expected-half-float.png`, `expected-macos.png`, ...); matching any one passes.
    let mut references = expected_images(test_dir)?;
    if references.is_empty() {
        return Err(format!(
            "expected.png not found: {}",
            test_dir.join("expected.png").display()
        ));
    }
    let diff_path = test_dir.join("diff.png");
    let mut best: Option<(Difference, PathBuf)> = None;
    for reference in references.drain(..) {
        let difference = compare_and_diff(&actual, &reference, &diff_path, meta.pixel_ratio)
            .map_err(|error| format!("Image comparison failed: {error}"))?;
        if best
            .as_ref()
            .is_none_or(|(closest, _)| difference.mean < closest.mean)
        {
            best = Some((difference, reference));
        }
    }
    let Some((difference, reference)) = best else {
        return Err("no reference image".into());
    };
    // Leave the difference image of the closest reference.
    compare_and_diff(&actual, &reference, &diff_path, meta.pixel_ratio)
        .map_err(|error| format!("Image comparison failed: {error}"))?;
    Ok(difference)
}

/// Every reference image of a fixture, `expected.png` first.
fn expected_images(test_dir: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = std::fs::read_dir(test_dir)
        .map_err(|error| format!("Cannot list {}: {error}", test_dir.display()))?;
    let mut references: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("expected") && name.ends_with(".png"))
        })
        .collect();
    references.sort();
    Ok(references)
}

pub(crate) mod camera_options;
#[cfg(test)]
mod tests;

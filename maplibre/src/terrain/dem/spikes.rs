//! Removal of the narrow elevation spikes that some DEM mosaics carry along source seams.
//!
//! Resampling across a seam leaves runs of one or two samples above both neighbours, from tens to
//! hundreds of metres, down north–south columns of the tile. Two rules lower such a sample to the
//! median of its window:
//!
//! - A peak rule for the tallest spikes: a sample that rises more than [`SPIKE_SLOPE`] ground
//!   sample distances above its row or column median.
//! - A seam rule for the lower rest of a seam: a sample that rises more than [`SEAM_FLOOR_SAMPLES`]
//!   ground sample distances, and at least [`SEAM_FLOOR_METRES`], above its east–west median,
//!   where such rises continue down the same column for at least [`SEAM_ROWS`] rows.
//!
//! Both rules only lower samples. Coarse tiles, where one sample can hold a real isolated peak,
//! have thresholds of kilometres and keep their peaks. Narrow pits are kept: a gorge one sample
//! wide is real terrain, and a north–south gorge has the same column signature as the seams'
//! shallow ringing dips, which do not stand out against the horizon.

use std::f64::consts::PI;

use super::DemTile;
use crate::coords::WorldTileCoords;

/// Policy: how many ground sample distances a sample may rise above its window median before it
/// counts as a spike. Seam ringing is one or two samples wide and stands hundreds of metres proud.
/// For terrain to rise three sample distances above the median of its five-sample window, it must
/// climb steeper than atan(3), about 72°, on both flanks. Bare-earth DEMs average the ground over
/// each sample and hold no such slopes, so steep real relief stays untouched.
const SPIKE_SLOPE: f64 = 3.0;
/// Policy: the rise above the east–west median, in ground sample distances, from which a sample
/// can belong to a seam: a 45° climb on both flanks. Lower rises are left to real terrain.
const SEAM_FLOOR_SAMPLES: f64 = 1.0;
/// Policy: the smallest rise in metres that can belong to a seam. Fine tiles have samples of a
/// few metres, where steep real ridges reach the sample-based floor; the seams rise far above it.
const SEAM_FLOOR_METRES: f64 = 20.0;
/// Policy: the rows a seam's rise must run down one column, give or take [`SEAM_SLACK`]. A ridge
/// one or two samples wide does not stay within three columns for sixteen rows unless it runs
/// within 7° of due north, while the seams follow one column for a hundred rows or more.
const SEAM_ROWS: usize = 16;
/// Columns either side of a column that continue its seam: ringing wanders by one sample.
const SEAM_SLACK: usize = 1;
/// Samples on each side of a sample in its row and column windows. Two lets a spike two samples
/// wide stay a minority of its window.
const WINDOW_RADIUS: i64 = 2;
/// Encoded value range of the three 8-bit channels.
const MAX_ENCODED: f64 = 16_777_215.0;

/// Ground size in metres of one sample of a `dim`-sample DEM tile, at the tile's centre latitude.
pub(crate) fn sample_metres(coords: WorldTileCoords, dim: u32, circumference_meters: f64) -> f64 {
    let n = 2_f64.powi(i32::from(u8::from(coords.z)));
    let latitude = (PI * (1.0 - 2.0 * (f64::from(coords.y) + 0.5) / n))
        .sinh()
        .atan();
    circumference_meters * latitude.cos() / (n * f64::from(dim.max(1)))
}

/// Re-encodes elevations for the Mapbox and Terrarium layouts, whose channels weigh 256 times
/// the next one. Other unpack vectors cannot be inverted channel by channel.
struct Encoding {
    step: f64,
    base: f64,
}

impl Encoding {
    fn new([red, green, blue, base]: [f64; 4]) -> Option<Self> {
        let close = |a: f64, b: f64| (a - b).abs() <= 1e-9 * b.abs();
        (blue > 0.0 && close(green, blue * 256.0) && close(red, blue * 65_536.0))
            .then_some(Self { step: blue, base })
    }

    fn channels(&self, elevation: f64) -> [u8; 3] {
        let value = ((elevation + self.base) / self.step)
            .round()
            .clamp(0.0, MAX_ENCODED) as u32;
        [(value >> 16) as u8, (value >> 8) as u8, value as u8]
    }
}

/// Median of up to `2 * WINDOW_RADIUS + 1` samples, kept on the stack.
fn median(samples: impl Iterator<Item = f64>) -> f64 {
    let mut window = [0.0; 2 * WINDOW_RADIUS as usize + 1];
    let mut len = 0;
    for (slot, sample) in window.iter_mut().zip(samples) {
        *slot = sample;
        len += 1;
    }
    let window = &mut window[..len];
    window.sort_unstable_by(f64::total_cmp);
    let middle = len / 2;
    match len % 2 {
        0 if middle > 0 => (window[middle - 1] + window[middle]) / 2.0,
        _ => window.get(middle).copied().unwrap_or(f64::NAN),
    }
}

/// The median of a window when it lies below `ceiling`. A median can only lie below it when at
/// least half the window does, so counting first skips the sort for almost every sample: the filter
/// runs on every sample of every decoded tile.
fn median_below(samples: impl Iterator<Item = f64> + Clone, ceiling: f64) -> Option<f64> {
    let (below, len) = samples
        .clone()
        .fold((0_usize, 0_usize), |(below, len), sample| {
            (below + usize::from(sample < ceiling), len + 1)
        });
    (below >= len.div_ceil(2))
        .then(|| median(samples))
        .filter(|median| *median < ceiling)
}

/// Decoded interior samples of a tile, row by row.
struct Heights {
    dim: usize,
    samples: Vec<f64>,
}

impl Heights {
    fn at(&self, x: usize, y: usize) -> f64 {
        self.samples[y * self.dim + x]
    }

    fn window(&self, centre: usize) -> std::ops::RangeInclusive<usize> {
        let radius = WINDOW_RADIUS as usize;
        centre.saturating_sub(radius)..=(centre + radius).min(self.dim - 1)
    }

    /// The row median of `(x, y)` when the sample rises more than `rise` above it.
    fn row_median_below(&self, x: usize, y: usize, rise: f64) -> Option<f64> {
        median_below(self.window(x).map(|i| self.at(i, y)), self.at(x, y) - rise)
    }

    /// The column median of `(x, y)` when the sample rises more than `rise` above it.
    fn column_median_below(&self, x: usize, y: usize, rise: f64) -> Option<f64> {
        median_below(self.window(y).map(|i| self.at(x, i)), self.at(x, y) - rise)
    }
}

/// For every sample, the east–west median it rises above by more than the seam floor, if any.
fn seam_rises(heights: &Heights, floor: f64) -> Vec<Option<f64>> {
    let dim = heights.dim;
    (0..dim * dim)
        .map(|index| heights.row_median_below(index % dim, index / dim, floor))
        .collect()
}

/// Whether each risen sample lies on a run of risen rows at least [`SEAM_ROWS`] long within
/// [`SEAM_SLACK`] columns of it.
fn seam_columns(rises: &[Option<f64>], dim: usize) -> Vec<bool> {
    let near = |x: usize, y: usize| {
        (x.saturating_sub(SEAM_SLACK)..=(x + SEAM_SLACK).min(dim - 1))
            .any(|column| rises[y * dim + column].is_some())
    };
    let mut seam = vec![false; dim * dim];
    for x in 0..dim {
        let mut start = 0;
        for y in 0..=dim {
            if y < dim && near(x, y) {
                continue;
            }
            if y - start >= SEAM_ROWS {
                for row in start..y {
                    seam[row * dim + x] = rises[row * dim + x].is_some();
                }
            }
            start = y + 1;
        }
    }
    seam
}

impl DemTile {
    /// Lowers the interior samples that are spikes for a ground sample distance of
    /// `sample_metres` and returns how many changed. Borders are left to the caller.
    pub(super) fn remove_spikes(&mut self, sample_metres: f64) -> usize {
        let Some(encoding) = Encoding::new(self.unpack) else {
            return 0;
        };
        if !(sample_metres.is_finite() && sample_metres > 0.0) || self.dim == 0 {
            return 0;
        }
        let dim = self.dim as usize;
        let heights = Heights {
            dim,
            samples: (0..dim as i64)
                .flat_map(|y| (0..dim as i64).map(move |x| (x, y)))
                .map(|(x, y)| self.get(x, y))
                .collect(),
        };
        let peak_rise = SPIKE_SLOPE * sample_metres;
        let seam_floor = (SEAM_FLOOR_SAMPLES * sample_metres).max(SEAM_FLOOR_METRES);
        let rises = seam_rises(&heights, seam_floor);
        let seams = seam_columns(&rises, dim);
        let mut replaced = 0;
        for y in 0..dim {
            for x in 0..dim {
                let peak = heights
                    .row_median_below(x, y, peak_rise)
                    .into_iter()
                    .chain(heights.column_median_below(x, y, peak_rise))
                    .reduce(f64::min);
                let seam = rises[y * dim + x].filter(|_| seams[y * dim + x]);
                if let Some(median) = peak.into_iter().chain(seam).reduce(f64::min) {
                    let index = self.byte_index(x as i64, y as i64);
                    self.pixels[index..index + 3].copy_from_slice(&encoding.channels(median));
                    replaced += 1;
                }
            }
        }
        replaced
    }
}

#[cfg(test)]
mod tests;

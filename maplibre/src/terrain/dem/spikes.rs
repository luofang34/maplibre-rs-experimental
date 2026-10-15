//! Removal of the narrow elevation spikes that some DEM mosaics carry along source seams.
//!
//! Resampling across a seam leaves runs of one or two samples hundreds of metres above both
//! neighbours. Bare-earth terrain is not that steep on both flanks, so a sample that rises above
//! the median of its row or column window by more than [`SPIKE_SLOPE`] ground sample distances
//! takes that median. Coarse tiles, where one sample can hold a real isolated peak, have a
//! threshold of kilometres and keep their peaks. Narrow pits are kept: a gorge one sample wide is
//! real terrain, and the seams' shallow dips do not stand out against the horizon.

use std::f64::consts::PI;

use super::DemTile;
use crate::coords::WorldTileCoords;

/// Policy: how many ground sample distances a sample may rise above its window median before it
/// counts as a spike. Seam ringing is one or two samples wide and stands hundreds of metres proud.
/// For terrain to rise three sample distances above the median of its five-sample window, it must
/// climb steeper than atan(3), about 72°, on both flanks. Bare-earth DEMs average the ground over
/// each sample and hold no such slopes, so steep real relief stays untouched.
const SPIKE_SLOPE: f64 = 3.0;
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

impl DemTile {
    /// Replaces the interior samples that are spikes for a ground sample distance of
    /// `sample_metres` and returns how many changed. Borders are left to the caller.
    pub(super) fn remove_spikes(&mut self, sample_metres: f64) -> usize {
        let Some(encoding) = Encoding::new(self.unpack) else {
            return 0;
        };
        if !(sample_metres.is_finite() && sample_metres > 0.0) {
            return 0;
        }
        let threshold = SPIKE_SLOPE * sample_metres;
        let dim = i64::from(self.dim);
        let heights: Vec<f64> = (0..dim)
            .flat_map(|y| (0..dim).map(move |x| (x, y)))
            .map(|(x, y)| self.get(x, y))
            .collect();
        let at = |x: i64, y: i64| heights[(y * dim + x) as usize];
        let window =
            |centre: i64| (centre - WINDOW_RADIUS).max(0)..=(centre + WINDOW_RADIUS).min(dim - 1);
        let mut replaced = 0;
        for y in 0..dim {
            for x in 0..dim {
                let ceiling = at(x, y) - threshold;
                let row = median_below(window(x).map(|i| at(i, y)), ceiling);
                let column = median_below(window(y).map(|i| at(x, i)), ceiling);
                if let Some(median) = row.into_iter().chain(column).reduce(f64::min) {
                    let index = self.byte_index(x, y);
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

//! Decoded elevation tiles.

use image::RgbaImage;
use thiserror::Error;

use crate::coords::EXTENT;

/// Failure while building a DEM tile from an image.
#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum DemError {
    /// Elevation tiles must be square.
    #[error("DEM tiles must be square, got {width}x{height} pixels")]
    NotSquare {
        /// Image width in pixels.
        width: u32,
        /// Image height in pixels.
        height: u32,
    },
    /// Elevation tiles must contain at least one sample.
    #[error("DEM tiles must not be empty")]
    Empty,
    /// Neighbouring tiles must have the same number of samples to share borders.
    #[error("DEM neighbour has {actual} samples per edge, expected {expected}")]
    DimensionMismatch {
        /// Samples per edge of this tile.
        expected: u32,
        /// Samples per edge of the neighbour.
        actual: u32,
    },
}

/// Elevation samples of one tile with a one-pixel border, following GL JS `DEMData`.
///
/// Pixels keep their RGB encoding so the same bytes upload to the GPU unchanged; the unpack
/// vector turns a channel triple into metres on both sides. The border replicates the nearest
/// edge sample so interpolation stays continuous until neighbours can fill it.
#[derive(Clone, Debug, PartialEq)]
pub struct DemTile {
    dim: u32,
    stride: u32,
    pixels: Vec<u8>,
    unpack: [f64; 4],
    min: f64,
    max: f64,
}

impl DemTile {
    /// Builds a tile from a decoded image and the source's unpack vector.
    pub fn from_image(image: &RgbaImage, unpack: [f64; 4]) -> Result<Self, DemError> {
        let (width, height) = image.dimensions();
        if width != height {
            return Err(DemError::NotSquare { width, height });
        }
        if width == 0 {
            return Err(DemError::Empty);
        }
        let dim = width;
        let stride = dim + 2;
        let mut tile = Self {
            dim,
            stride,
            pixels: vec![0; (stride * stride * 4) as usize],
            unpack,
            min: f64::INFINITY,
            max: f64::NEG_INFINITY,
        };
        let source = image.as_raw();
        for y in 0..dim {
            let source_row = (y * dim * 4) as usize;
            let target_row = tile.byte_index(0, i64::from(y));
            tile.pixels[target_row..target_row + (dim * 4) as usize]
                .copy_from_slice(&source[source_row..source_row + (dim * 4) as usize]);
        }
        tile.replicate_border();
        for y in 0..dim {
            for x in 0..dim {
                let elevation = tile.get(i64::from(x), i64::from(y));
                tile.min = tile.min.min(elevation);
                tile.max = tile.max.max(elevation);
            }
        }
        Ok(tile)
    }

    fn replicate_border(&mut self) {
        let last = i64::from(self.dim) - 1;
        let edge = i64::from(self.dim);
        for i in 0..i64::from(self.dim) {
            self.copy_pixel((0, i), (-1, i));
            self.copy_pixel((last, i), (edge, i));
            self.copy_pixel((i, 0), (i, -1));
            self.copy_pixel((i, last), (i, edge));
        }
        self.copy_pixel((0, 0), (-1, -1));
        self.copy_pixel((last, 0), (edge, -1));
        self.copy_pixel((0, last), (-1, edge));
        self.copy_pixel((last, last), (edge, edge));
    }

    fn copy_pixel(&mut self, from: (i64, i64), to: (i64, i64)) {
        let from = self.byte_index(from.0, from.1);
        let to = self.byte_index(to.0, to.1);
        self.pixels.copy_within(from..from + 4, to);
    }

    /// Byte offset of a sample, where `-1` and `dim` address the border.
    fn byte_index(&self, x: i64, y: i64) -> usize {
        let stride = i64::from(self.stride);
        (((y + 1) * stride + (x + 1)) * 4) as usize
    }

    /// Elevation in metres of one sample, where `-1` and `dim` address the border.
    pub fn get(&self, x: i64, y: i64) -> f64 {
        let x = x.clamp(-1, i64::from(self.dim));
        let y = y.clamp(-1, i64::from(self.dim));
        let index = self.byte_index(x, y);
        let [red, green, blue, base] = self.unpack;
        f64::from(self.pixels[index]) * red
            + f64::from(self.pixels[index + 1]) * green
            + f64::from(self.pixels[index + 2]) * blue
            - base
    }

    /// Bilinear elevation at sample-space coordinates in `0..=dim`.
    ///
    /// Matches the vertex shader, which treats sample `i` as sitting at coordinate `i` and blends
    /// towards the next sample or the border.
    pub fn sample_bilinear(&self, x: f64, y: f64) -> f64 {
        let x = x.clamp(0.0, f64::from(self.dim));
        let y = y.clamp(0.0, f64::from(self.dim));
        let cx = x.floor();
        let cy = y.floor();
        let (tx, ty) = (x - cx, y - cy);
        let (cx, cy) = (cx as i64, cy as i64);
        let top = self.get(cx, cy) * (1.0 - tx) + self.get(cx + 1, cy) * tx;
        let bottom = self.get(cx, cy + 1) * (1.0 - tx) + self.get(cx + 1, cy + 1) * tx;
        top * (1.0 - ty) + bottom * ty
    }

    /// Elevation at tile coordinates in `0..=EXTENT`.
    pub fn elevation_at_tile_coords(&self, x: f64, y: f64) -> f64 {
        let scale = f64::from(self.dim) / EXTENT;
        self.sample_bilinear(x * scale, y * scale)
    }

    /// Replaces the border facing a neighbour at `(dx, dy)` with that neighbour's edge samples.
    pub fn backfill_border(
        &mut self,
        neighbour: &DemTile,
        dx: i32,
        dy: i32,
    ) -> Result<(), DemError> {
        if neighbour.dim != self.dim {
            return Err(DemError::DimensionMismatch {
                expected: self.dim,
                actual: neighbour.dim,
            });
        }
        self.fill_border(dx, dy, &neighbour.edge_samples(dx, dy));
        Ok(())
    }

    /// Samples of this tile that a neighbour at `(-dx, -dy)` stores in its border facing us.
    ///
    /// Row-major over that neighbour's border region: a column for `dx != 0`, a row for
    /// `dy != 0`, a single corner sample when both are set.
    pub fn edge_samples(&self, dx: i32, dy: i32) -> Vec<u8> {
        let dim = i64::from(self.dim);
        let (x_range, y_range) = Self::border_region(dim, dx, dy);
        let count = (x_range.end - x_range.start) * (y_range.end - y_range.start);
        let mut samples = Vec::with_capacity(count.max(0) as usize * 4);
        for y in y_range {
            for x in x_range.clone() {
                let index = self.byte_index(x - i64::from(dx) * dim, y - i64::from(dy) * dim);
                samples.extend_from_slice(&self.pixels[index..index + 4]);
            }
        }
        samples
    }

    /// Writes samples produced by a neighbour's [`edge_samples`](Self::edge_samples) into the
    /// border facing that neighbour at `(dx, dy)`.
    pub fn fill_border(&mut self, dx: i32, dy: i32, samples: &[u8]) {
        let dim = i64::from(self.dim);
        let (x_range, y_range) = Self::border_region(dim, dx, dy);
        let mut source = samples.chunks_exact(4);
        for y in y_range {
            for x in x_range.clone() {
                let Some(pixel) = source.next() else {
                    return;
                };
                let index = self.byte_index(x, y);
                self.pixels[index..index + 4].copy_from_slice(pixel);
            }
        }
    }

    /// Border cells facing a neighbour at `(dx, dy)`, as in GL JS `backfillBorder`.
    fn border_region(dim: i64, dx: i32, dy: i32) -> (std::ops::Range<i64>, std::ops::Range<i64>) {
        let axis = |delta: i32| match delta {
            -1 => -1..0,
            1 => dim..dim + 1,
            _ => 0..dim,
        };
        (axis(dx), axis(dy))
    }

    /// Number of samples along one edge, excluding the border.
    pub fn dim(&self) -> u32 {
        self.dim
    }

    /// Number of pixels along one edge of the bordered image.
    pub fn stride(&self) -> u32 {
        self.stride
    }

    /// Bordered RGBA pixels, `stride * stride * 4` bytes.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Channel factors and base shift used to decode the pixels.
    pub fn unpack(&self) -> [f64; 4] {
        self.unpack
    }

    /// Lowest interior elevation in metres.
    pub fn min(&self) -> f64 {
        self.min
    }

    /// Highest interior elevation in metres.
    pub fn max(&self) -> f64 {
        self.max
    }
}

#[cfg(test)]
mod tests;

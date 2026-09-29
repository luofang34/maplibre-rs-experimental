//! Decoded elevation tiles.

use image::RgbaImage;
use thiserror::Error;

use crate::coords::EXTENT;

/// Failure while decoding a DEM tile or filling its border.
#[derive(Clone, Copy, Debug, Error, PartialEq)]
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
    /// A border must face one of the eight adjacent tiles.
    #[error("invalid DEM neighbour offset ({dx}, {dy})")]
    InvalidNeighbour {
        /// Horizontal offset.
        dx: i32,
        /// Vertical offset.
        dy: i32,
    },
    /// Encoded samples cannot be copied between differently encoded tiles.
    #[error("DEM neighbour unpack vector {actual:?} differs from {expected:?}")]
    EncodingMismatch {
        /// This tile's channel factors and base shift.
        expected: [f64; 4],
        /// The neighbour's channel factors and base shift.
        actual: [f64; 4],
    },
    /// The border must be supplied in full to avoid partially updated samples.
    #[error("DEM border ({dx}, {dy}) requires {expected} bytes, got {actual}")]
    BorderLength {
        /// Horizontal offset.
        dx: i32,
        /// Vertical offset.
        dy: i32,
        /// Required byte count.
        expected: usize,
        /// Supplied byte count.
        actual: usize,
    },
}

/// Elevation samples of one tile with a two-pixel border, following GL JS `DEMData`.
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
        let stride = dim + 4;
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
        let dim = i64::from(self.dim);
        for y in -2..dim + 2 {
            if y < 0 || y >= dim {
                let from = self.byte_index(0, y.clamp(0, dim - 1));
                let to = self.byte_index(0, y);
                self.pixels
                    .copy_within(from..from + self.dim as usize * 4, to);
            }
            for x in [-2, -1, dim, dim + 1] {
                self.copy_pixel((x.clamp(0, dim - 1), y), (x, y));
            }
        }
    }

    fn copy_pixel(&mut self, from: (i64, i64), to: (i64, i64)) {
        let from = self.byte_index(from.0, from.1);
        let to = self.byte_index(to.0, to.1);
        self.pixels.copy_within(from..from + 4, to);
    }

    /// Byte offset in the two-pixel padded image.
    fn byte_index(&self, x: i64, y: i64) -> usize {
        let stride = i64::from(self.stride);
        (((y + 2) * stride + (x + 2)) * 4) as usize
    }

    /// Elevation in metres, clamped to the padded range `-2..=dim + 1`.
    pub fn get(&self, x: i64, y: i64) -> f64 {
        let x = x.clamp(-2, i64::from(self.dim) + 1);
        let y = y.clamp(-2, i64::from(self.dim) + 1);
        let index = self.byte_index(x, y);
        let [red, green, blue, base] = self.unpack;
        f64::from(self.pixels[index]) * red
            + f64::from(self.pixels[index + 1]) * green
            + f64::from(self.pixels[index + 2]) * blue
            - base
    }

    /// Bilinear elevation in sample space, clamped to `-1..=dim`.
    ///
    /// Integer coordinates address pixel centres; negative fractions interpolate the west or
    /// north border with the first interior sample.
    pub fn sample_bilinear(&self, x: f64, y: f64) -> f64 {
        let x = x.clamp(-1.0, f64::from(self.dim));
        let y = y.clamp(-1.0, f64::from(self.dim));
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
        self.sample_bilinear(
            x.clamp(0.0, EXTENT) * scale - 0.5,
            y.clamp(0.0, EXTENT) * scale - 0.5,
        )
    }

    /// Replaces the border facing a neighbour at `(dx, dy)` with that neighbour's edge samples.
    pub fn backfill_border(
        &mut self,
        neighbour: &DemTile,
        dx: i32,
        dy: i32,
    ) -> Result<(), DemError> {
        self.validate_neighbour(neighbour.dim, neighbour.unpack)?;
        self.fill_border(dx, dy, &neighbour.edge_samples(dx, dy)?)?;
        Ok(())
    }

    pub(super) fn validate_neighbour(&self, dim: u32, unpack: [f64; 4]) -> Result<(), DemError> {
        if dim != self.dim {
            return Err(DemError::DimensionMismatch {
                expected: self.dim,
                actual: dim,
            });
        }
        if unpack != self.unpack {
            return Err(DemError::EncodingMismatch {
                expected: self.unpack,
                actual: unpack,
            });
        }
        Ok(())
    }

    /// Samples of this tile that a neighbour at `(-dx, -dy)` stores in its border facing us.
    ///
    /// Row-major over two columns or rows, or a 2x2 corner when both offsets are set.
    pub fn edge_samples(&self, dx: i32, dy: i32) -> Result<Vec<u8>, DemError> {
        let dim = i64::from(self.dim);
        let (x_range, y_range) = Self::border_region(dim, dx, dy)?;
        let count = (x_range.end - x_range.start) * (y_range.end - y_range.start);
        let mut samples = Vec::with_capacity(count.max(0) as usize * 4);
        for y in y_range {
            for x in x_range.clone() {
                let index = self.byte_index(x - i64::from(dx) * dim, y - i64::from(dy) * dim);
                samples.extend_from_slice(&self.pixels[index..index + 4]);
            }
        }
        Ok(samples)
    }

    /// Writes samples produced by a neighbour's [`edge_samples`](Self::edge_samples) into the
    /// border facing that neighbour at `(dx, dy)`.
    /// Returns whether any pixels changed.
    pub fn fill_border(&mut self, dx: i32, dy: i32, samples: &[u8]) -> Result<bool, DemError> {
        let dim = i64::from(self.dim);
        let (x_range, y_range) = Self::border_region(dim, dx, dy)?;
        let expected =
            (x_range.end - x_range.start) as usize * (y_range.end - y_range.start) as usize * 4;
        if samples.len() != expected {
            return Err(DemError::BorderLength {
                dx,
                dy,
                expected,
                actual: samples.len(),
            });
        }
        let mut changed = false;
        let positions = y_range.flat_map(|y| x_range.clone().map(move |x| (x, y)));
        for ((x, y), pixel) in positions.zip(samples.chunks_exact(4)) {
            let index = self.byte_index(x, y);
            let target = &mut self.pixels[index..index + 4];
            changed |= target != pixel;
            target.copy_from_slice(pixel);
        }
        Ok(changed)
    }

    fn border_region(
        dim: i64,
        dx: i32,
        dy: i32,
    ) -> Result<(std::ops::Range<i64>, std::ops::Range<i64>), DemError> {
        if !(-1..=1).contains(&dx) || !(-1..=1).contains(&dy) || (dx == 0 && dy == 0) {
            return Err(DemError::InvalidNeighbour { dx, dy });
        }
        let axis = |delta: i32| match delta {
            -1 => -2..0,
            1 => dim..dim + 2,
            _ => 0..dim,
        };
        Ok((axis(dx), axis(dy)))
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

#[cfg(all(test, not(target_arch = "wasm32")))]
pub(crate) mod gpu_readback;

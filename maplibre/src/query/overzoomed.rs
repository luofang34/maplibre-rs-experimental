//! The features of a tile cut out of a coarser one, as GL JS `sliceVectorTileLayer` cuts an
//! overzoomed tile out of the last tile its source has.

use crate::{coords::EXTENT, io::geometry_index::IndexedGeometry};

/// The buffer GL JS keeps around an overzoomed tile, in the units of its 8192-unit extent.
const GL_BUFFER: f64 = 128.0;

/// The parts of a coarser tile's geometry within a finer tile `scale` times smaller whose top
/// left corner lies at `offset` of the finer tile's units: moved into its units, cut to it and its
/// buffer, and left out where only the buffer holds them, as GL JS `FeatureIndex.insert` leaves
/// them to the neighbouring tile.
pub(super) fn slice(
    geometry: &IndexedGeometry<f64>,
    scale: f64,
    offset: [f64; 2],
) -> Vec<IndexedGeometry<f64>> {
    let buffer = GL_BUFFER * EXTENT / 8192.0;
    let area = geo_types::Rect::new(
        geo_types::Coord {
            x: -buffer,
            y: -buffer,
        },
        geo_types::Coord {
            x: EXTENT + buffer,
            y: EXTENT + buffer,
        },
    );
    geometry
        .rescaled(scale, offset)
        .clipped_to(area)
        .into_iter()
        .filter(|part| {
            let (lower, upper) = (part.bounds.lower(), part.bounds.upper());
            lower.x() < EXTENT && lower.y() < EXTENT && upper.x() >= 0.0 && upper.y() >= 0.0
        })
        .collect()
}

#[cfg(test)]
mod tests;

//! Loaded tile coverage and per-frame metadata for stencil masks and tile draws.

#![deny(missing_docs, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod availability;
pub(crate) mod coverage;
mod pattern;

use std::{mem::size_of, ops::Range};

pub(crate) use availability::SourceTiles;
pub use availability::{HasTile, KindSources, QueryHasTile, ViewTileSources};
use cgmath::Matrix4;
pub use pattern::{
    covering_shapes_for, RasterCoverings, TileMetadataOverflow, TileViewPattern,
    COMPLETE_CHILDREN_SEARCH_DEPTH, DEFAULT_TILE_VIEW_PATTERN_SIZE,
};

use crate::{
    coords::{WorldTileCoords, Zoom},
    io::tile_sources::TileKind,
    raster::RasterSourceId,
    render::shaders::ShaderTileMetadata,
};

/// Tile pattern backed by a wgpu metadata buffer and queue.
pub type WgpuTileViewPattern =
    TileViewPattern<crate::render::upload_queue::UploadQueue, wgpu::Buffer>;

/// If not otherwise specified, raster tiles usually are 512.0 by 512.0 pixel.
/// In order to support 256.0 x 256.0 raster tiles 256.0 must be used.
///
/// Vector tiles always have a size of 512.0.
pub const DEFAULT_TILE_SIZE: f64 = 512.0;

/// This defines the source tile shaped from which the content for the `target` is taken.
/// For example if the target is `(0, 0, 1)` (of [`ViewTile`]) , we might use
/// `SourceShapes::Parent((0, 0, 0))` as source.
/// Similarly if we have the target `(0, 0, 0)` we might use
/// `SourceShapes::Children((0, 0, 1), (0, 1, 1), (1, 0, 1), (1, 1, 1))` as sources.
#[derive(Debug, Clone)]
pub enum SourceShapes {
    /// Parent tile is the source. We construct the `target` from parts of a parent.
    Parent(TileShape),
    /// Children are the source. We construct the `target` from multiple children.
    Children(Vec<TileShape>),
    /// Source and target are equal, so no need to differentiate. We render the `source` shape
    /// exactly at the `target`.
    SourceEqTarget(TileShape),
    /// No data available so nothing to render
    None,
}

impl SourceShapes {
    fn for_each(&self, callback: &mut impl FnMut(&TileShape)) {
        match self {
            Self::Parent(source_shape) | Self::SourceEqTarget(source_shape) => {
                callback(source_shape)
            }
            Self::Children(source_shapes) => {
                for shape in source_shapes {
                    callback(shape)
                }
            }
            Self::None => {}
        }
    }

    fn for_each_mut(&mut self, callback: &mut impl FnMut(&mut TileShape)) {
        match self {
            Self::Parent(source_shape) | Self::SourceEqTarget(source_shape) => {
                callback(source_shape)
            }
            Self::Children(source_shapes) => {
                for shape in source_shapes {
                    callback(shape)
                }
            }
            Self::None => {}
        }
    }
}

/// Defines the `target` tile and the sources its data comes from. Vector and raster sources
/// have shapes of their own, as GL JS gives every source its own tile manager: a 256-pixel
/// raster source draws its four children into a view tile whose vector tile is the tile
/// itself.
#[derive(Debug, Clone)]
pub struct ViewTile {
    target: WorldTileCoords,
    vector: SourceShapes,
    raster: Vec<(RasterSourceId, SourceShapes)>,
}

impl ViewTile {
    /// Tile coordinate represented by this entry.
    pub fn coords(&self) -> WorldTileCoords {
        self.target
    }

    /// Visits vector shapes and each raster source's shapes, including shared coordinates.
    pub fn render<F>(&self, mut callback: F)
    where
        F: FnMut(&TileShape),
    {
        self.vector.for_each(&mut callback);
        for (_, shapes) in &self.raster {
            shapes.for_each(&mut callback);
        }
    }

    /// Visits the shapes drawn for one kind of source.
    pub fn render_kind<F>(&self, kind: TileKind, mut callback: F)
    where
        F: FnMut(&TileShape),
    {
        match kind {
            TileKind::Vector => self.vector.for_each(&mut callback),
            TileKind::Raster => {
                for (_, shapes) in &self.raster {
                    shapes.for_each(&mut callback);
                }
            }
        }
    }

    /// Visits only the shapes owned by one raster source.
    pub fn render_raster_source(
        &self,
        source: &RasterSourceId,
        mut callback: impl FnMut(&TileShape),
    ) {
        for (_, shapes) in self.raster.iter().filter(|(id, _)| id == source) {
            shapes.for_each(&mut callback);
        }
    }
}

/// Defines the exact location where a specific tile on the map is rendered. It defines the shape
/// of the tile with its location for the current zoom factor.
#[derive(Debug, Clone)]
pub struct TileShape {
    coords: WorldTileCoords,
    /// How many worlds from the tile's own this shape is drawn, for a copy of the world.
    pub(crate) wrap: i32,

    zoom_factor: f64,
    transform: Matrix4<f64>,

    buffer_range: Option<Range<wgpu::BufferAddress>>,
    /// The tile whose stencil mask this shape draws within, when it is drawn over that tile
    /// rather than in its own place in the pyramid.
    within: Option<WorldTileCoords>,
}

impl TileShape {
    fn new(coords: WorldTileCoords, zoom: Zoom) -> Self {
        Self {
            coords,
            wrap: 0,
            zoom_factor: zoom.scale_to_tile(&coords),
            transform: coords.transform_for_zoom(zoom),
            buffer_range: None,
            within: None,
        }
    }

    /// The shape drawn inside `tile`'s mask, over that tile.
    fn within(mut self, tile: WorldTileCoords) -> Self {
        self.within = Some(tile);
        self
    }

    /// The tile whose stencil mask the shape draws within: its own, unless it is drawn over
    /// another tile.
    pub fn stencil_coords(&self) -> WorldTileCoords {
        self.within.unwrap_or(self.coords)
    }

    /// Whether the shape is drawn within another tile's mask and so needs none of its own.
    pub fn is_drawn_within_another(&self) -> bool {
        self.within.is_some()
    }

    /// Creates a shape whose metadata lives at an explicit buffer range, such as a drape entry.
    pub(crate) fn with_buffer_range(
        coords: WorldTileCoords,
        zoom: Zoom,
        buffer_range: Range<wgpu::BufferAddress>,
    ) -> Self {
        let mut shape = Self::new(coords, zoom);
        shape.buffer_range = Some(buffer_range);
        shape
    }

    /// The shape drawn `wrap` worlds over, where the view sees a copy of the world.
    fn wrapped(&mut self, wrap: i32, zoom: Zoom) {
        let tiles = 1_i64 << u32::from(u8::from(self.coords.z));
        let placed = WorldTileCoords {
            x: (i64::from(self.coords.x) + i64::from(wrap) * tiles) as i32,
            ..self.coords
        };
        self.wrap = wrap;
        self.transform = placed.transform_for_zoom(zoom);
    }

    fn set_buffer_range(&mut self, index: u64) {
        const STRIDE: u64 = size_of::<ShaderTileMetadata>() as u64;
        self.buffer_range = Some(index * STRIDE..(index + 1) * STRIDE);
    }

    fn clear_buffer_range(&mut self) {
        self.buffer_range = None;
    }

    /// Byte range populated by the latest upload, or `None` before upload or on overflow.
    /// The range is valid until the pattern buffer is uploaded again.
    pub fn buffer_range(&self) -> Option<Range<wgpu::BufferAddress>> {
        self.buffer_range.clone()
    }

    /// Tile coordinate represented by this entry.
    pub fn coords(&self) -> WorldTileCoords {
        self.coords
    }

    /// Whether both shapes draw the same tile at the same place. A world copy of a tile has
    /// the tile's coordinates but another place, and a parent standing in for several view
    /// tiles is one shape however many entries carry it.
    pub fn same_placement(&self, other: &Self) -> bool {
        self.coords == other.coords && self.transform == other.transform
    }
}

impl Default for TileShape {
    fn default() -> Self {
        Self::new(
            crate::coords::WorldTileCoords::default(),
            crate::coords::Zoom::default(),
        )
    }
}

#[cfg(test)]
mod tests;

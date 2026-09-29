//! Loaded tile coverage and per-frame metadata for stencil masks and tile draws.

#![deny(missing_docs, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

pub(crate) mod coverage;

mod pattern;

use std::{marker::PhantomData, mem::size_of, ops::Range};

use cgmath::Matrix4;
pub use pattern::{
    covering_shapes_for, RasterCoverings, TileMetadataOverflow, TileViewPattern,
    COMPLETE_CHILDREN_SEARCH_DEPTH, DEFAULT_TILE_VIEW_PATTERN_SIZE,
};

use crate::{
    coords::{WorldTileCoords, Zoom},
    io::tile_sources::TileKind,
    render::shaders::ShaderTileMetadata,
    tcs::{resources::ResourceQuery, world::World},
};

/// Tile pattern backed by a wgpu metadata buffer and queue.
pub type WgpuTileViewPattern = TileViewPattern<wgpu::Queue, wgpu::Buffer>;

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
    raster: SourceShapes,
}

impl ViewTile {
    /// Tile coordinate represented by this entry.
    pub fn coords(&self) -> WorldTileCoords {
        self.target
    }

    /// Visits the shapes of every kind; a tile serving both kinds is visited once per kind.
    pub fn render<F>(&self, mut callback: F)
    where
        F: FnMut(&TileShape),
    {
        self.vector.for_each(&mut callback);
        self.raster.for_each(&mut callback);
    }

    /// Visits the shapes drawn for one kind of source.
    pub fn render_kind<F>(&self, kind: TileKind, mut callback: F)
    where
        F: FnMut(&TileShape),
    {
        self.shapes(kind).for_each(&mut callback);
    }

    fn shapes(&self, kind: TileKind) -> &SourceShapes {
        match kind {
            TileKind::Vector => &self.vector,
            TileKind::Raster => &self.raster,
        }
    }
}

/// Defines the exact location where a specific tile on the map is rendered. It defines the shape
/// of the tile with its location for the current zoom factor.
#[derive(Debug, Clone)]
pub struct TileShape {
    coords: WorldTileCoords,

    zoom_factor: f64,
    transform: Matrix4<f64>,

    buffer_range: Option<Range<wgpu::BufferAddress>>,
}

impl TileShape {
    fn new(coords: WorldTileCoords, zoom: Zoom) -> Self {
        Self {
            coords,
            zoom_factor: zoom.scale_to_tile(&coords),
            transform: coords.transform_for_zoom(zoom),
            buffer_range: None,
        }
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
}

impl Default for TileShape {
    fn default() -> Self {
        Self::new(
            crate::coords::WorldTileCoords::default(),
            crate::coords::Zoom::default(),
        )
    }
}

/// Availability provider used to choose loaded replacements for requested tiles.
/// Missing or uninitialized backing resources must report tiles as unavailable.
pub trait HasTile {
    /// Whether this tile has the resources required by the provider to render.
    fn has_tile(&self, coords: WorldTileCoords, world: &World) -> bool;

    /// Finds the nearest available ancestor, including `coords` itself; returns `None` at an empty root.
    fn get_available_parent(
        &self,
        coords: WorldTileCoords,
        world: &World,
    ) -> Option<WorldTileCoords> {
        let mut current = coords;
        loop {
            if self.has_tile(current, world) {
                return Some(current);
            } else {
                let parent = current.get_parent()?;
                current = parent
            }
        }
    }

    /// Loaded descendants that cover `coords` completely, at most `search_depth` levels down.
    ///
    /// Returns `None` at depth zero or if any quadrant lacks data within the search depth.
    /// Coordinates must allow subdivision through the requested depth without integer overflow.
    fn get_complete_children(
        &self,
        coords: WorldTileCoords,
        world: &World,
        search_depth: usize,
    ) -> Option<Vec<WorldTileCoords>> {
        if search_depth == 0 {
            return None;
        }
        let mut output = Vec::with_capacity(4);
        for child in coords.get_children() {
            if self.has_tile(child, world) {
                output.push(child);
            } else {
                output.extend(self.get_complete_children(child, world, search_depth - 1)?);
            }
        }
        Some(output)
    }

    /// Finds available descendants up to `search_depth` levels below `coords`.
    /// Stops descending each loaded branch; coverage can be partial and an empty result is `Some([])`.
    /// Coordinates must allow subdivision through the requested depth without integer overflow.
    fn get_available_children(
        &self,
        coords: WorldTileCoords,
        world: &World,
        search_depth: usize,
    ) -> Option<Vec<WorldTileCoords>> {
        let mut children = coords.get_children().to_vec();

        let mut output = Vec::new();

        for _ in 0..search_depth {
            let mut new_children = Vec::with_capacity(children.len() * 4);

            for child in children {
                if self.has_tile(child, world) {
                    output.push(child);
                } else {
                    new_children.extend(child.get_children())
                }
            }

            children = new_children;
        }

        Some(output)
    }
}

impl<A: HasTile> HasTile for &A {
    fn has_tile(&self, coords: WorldTileCoords, world: &World) -> bool {
        A::has_tile(*self, coords, world)
    }
}

impl<A: HasTile> HasTile for (A,) {
    fn has_tile(&self, coords: WorldTileCoords, world: &World) -> bool {
        self.0.has_tile(coords, world)
    }
}

impl<A: HasTile, B: HasTile> HasTile for (A, B) {
    fn has_tile(&self, coords: WorldTileCoords, world: &World) -> bool {
        self.0.has_tile(coords, world) && self.1.has_tile(coords, world)
    }
}

impl<A: HasTile, B: HasTile, C: HasTile> HasTile for (A, B, C) {
    fn has_tile(&self, coords: WorldTileCoords, world: &World) -> bool {
        self.0.has_tile(coords, world)
            && self.1.has_tile(coords, world)
            && self.2.has_tile(coords, world)
    }
}

/// Resolves a resource query for each availability check; absent resources return `false`.
pub struct QueryHasTile<Q> {
    phantom_q: PhantomData<Q>,
}

impl<Q: ResourceQuery> Default for QueryHasTile<Q> {
    fn default() -> Self {
        Self {
            phantom_q: Default::default(),
        }
    }
}

impl<Q: ResourceQuery> HasTile for QueryHasTile<Q>
where
    for<'a> Q::Item<'a>: HasTile,
{
    fn has_tile(&self, coords: WorldTileCoords, world: &World) -> bool {
        world
            .resources
            .query::<Q>()
            .is_some_and(|resources| resources.has_tile(coords, world))
    }
}

/// The providers that know whether a tile of each kind is loaded.
#[derive(Default)]
pub struct ViewTileSources {
    vector: Vec<Box<dyn HasTile>>,
    raster: Vec<Box<dyn HasTile>>,
}

impl ViewTileSources {
    /// Registers a provider for one kind of tile.
    pub fn add<H: HasTile + 'static + Default>(&mut self, kind: TileKind) -> &mut Self {
        self.items_mut(kind).push(Box::<H>::default());
        self
    }

    /// Registers a resource-backed provider for one kind of tile.
    pub fn add_resource_query<Q: ResourceQuery + 'static>(&mut self, kind: TileKind) -> &mut Self
    where
        for<'a> Q::Item<'a>: HasTile,
    {
        self.items_mut(kind)
            .push(Box::new(QueryHasTile::<Q>::default()));
        self
    }

    /// Forgets every provider, so every tile counts as loaded.
    pub fn clear(&mut self) {
        self.vector.clear();
        self.raster.clear();
    }

    /// The providers of one kind, answering as one: a tile is loaded when every provider of
    /// the kind has it, and everything counts as loaded while no provider is registered.
    pub fn of_kind(&self, kind: TileKind) -> KindSources<'_> {
        KindSources(match kind {
            TileKind::Vector => &self.vector,
            TileKind::Raster => &self.raster,
        })
    }

    fn items_mut(&mut self, kind: TileKind) -> &mut Vec<Box<dyn HasTile>> {
        match kind {
            TileKind::Vector => &mut self.vector,
            TileKind::Raster => &mut self.raster,
        }
    }
}

/// The providers of one tile kind.
pub struct KindSources<'a>(&'a [Box<dyn HasTile>]);

impl HasTile for KindSources<'_> {
    fn has_tile(&self, coords: WorldTileCoords, world: &World) -> bool {
        self.0.iter().all(|item| item.has_tile(coords, world))
    }
}

#[cfg(test)]
mod tests;
